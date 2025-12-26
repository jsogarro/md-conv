use gray_matter::{engine::YAML, Matter};
use pulldown_cmark::{html, Options, Parser};
use tracing::instrument;

use crate::config::FrontMatter;

/// Parsed Markdown document
#[derive(Debug)]
pub struct ParsedDocument {
    pub front_matter: FrontMatter,
    pub html_content: String,
    pub raw_markdown: String,
}

/// Parse a Markdown file, extracting front matter and converting to HTML
#[instrument(skip(content), fields(content_len = content.len()))]
pub fn parse_markdown(content: &str) -> anyhow::Result<ParsedDocument> {
    let matter = Matter::<YAML>::new();
    let parsed = matter.parse(content);

    // Extract front matter or use defaults
    let front_matter: FrontMatter = parsed
        .data
        .map(|d| d.deserialize())
        .transpose()
        .map_err(|e| anyhow::anyhow!("Invalid front matter YAML: {}", e))?
        .unwrap_or_default();

    tracing::debug!(
        title = ?front_matter.title,
        author = ?front_matter.author,
        "Parsed front matter"
    );

    // Convert Markdown to HTML with GFM extensions
    let options = Options::ENABLE_GFM
        | Options::ENABLE_TABLES
        | Options::ENABLE_FOOTNOTES
        | Options::ENABLE_STRIKETHROUGH
        | Options::ENABLE_TASKLISTS
        | Options::ENABLE_SMART_PUNCTUATION
        | Options::ENABLE_HEADING_ATTRIBUTES;

    let parser = Parser::new_ext(&parsed.content, options);
    let mut html_content = String::with_capacity(parsed.content.len() * 2);
    html::push_html(&mut html_content, parser);

    tracing::debug!(html_len = html_content.len(), "Generated HTML");

    Ok(ParsedDocument {
        front_matter,
        html_content,
        raw_markdown: parsed.content.to_string(),
    })
}

#[derive(serde::Deserialize)]
struct Notebook {
    cells: Vec<Cell>,
}

#[derive(serde::Deserialize)]
struct Cell {
    cell_type: String,
    source: Vec<String>,
}

/// Parse a Jupyter Notebook file, converting it to Markdown and then to HTML
#[instrument(skip(content), fields(content_len = content.len()))]
pub fn parse_notebook(content: &str) -> anyhow::Result<ParsedDocument> {
    let notebook: Notebook = serde_json::from_str(content)
        .map_err(|e| anyhow::anyhow!("Invalid Jupyter Notebook JSON: {}", e))?;

    let mut markdown = String::new();
    for cell in notebook.cells {
        match cell.cell_type.as_str() {
            "markdown" => {
                for line in cell.source {
                    markdown.push_str(&line);
                }
                markdown.push('\n');
            }
            "code" => {
                markdown.push_str("\n```python\n");
                for line in cell.source {
                    markdown.push_str(&line);
                }
                markdown.push_str("\n```\n");
            }
            _ => {}
        }
    }

    // Reuse parse_markdown on the generated string
    parse_markdown(&markdown)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_with_front_matter() {
        let content = r#"---
title: "Test Doc"
author: "Tester"
pdf_options:
  format: "A4"
  margin: "25mm"
---
# Hello World

This is a test."#;

        let doc = parse_markdown(content).unwrap();
        assert_eq!(doc.front_matter.title, Some("Test Doc".into()));
        assert_eq!(doc.front_matter.author, Some("Tester".into()));
        assert!(doc.html_content.contains("<h1>Hello World</h1>"));
    }

    #[test]
    fn test_parse_without_front_matter() {
        let content = "# Just Markdown\n\nNo front matter here.";
        let doc = parse_markdown(content).unwrap();
        assert!(doc.front_matter.title.is_none());
        assert!(doc.html_content.contains("<h1>Just Markdown</h1>"));
    }

    #[test]
    fn test_gfm_features() {
        let content = "- [x] Task done\n- [ ] Task pending\n\n~~strikethrough~~";
        let doc = parse_markdown(content).unwrap();
        assert!(doc.html_content.contains("checked"));
        assert!(doc.html_content.contains("<del>"));
    }

    #[test]
    fn test_tables() {
        let content = "| A | B |\n|---|---|\n| 1 | 2 |";
        let doc = parse_markdown(content).unwrap();
        assert!(doc.html_content.contains("<table>"));
        assert!(doc.html_content.contains("<th>A</th>"));
    }

    #[test]
    fn test_footnotes() {
        let content = "Text with footnote[^1].\n\n[^1]: Footnote content.";
        let doc = parse_markdown(content).unwrap();
        assert!(doc.html_content.contains("footnote"));
    }

    #[test]
    fn test_invalid_front_matter_errors() {
        let content = "---\ninvalid: [unclosed\n---\n# Test";
        let result = parse_markdown(content);
        assert!(result.is_err());
    }
}
