use gray_matter::{engine::YAML, Matter};
use once_cell::sync::Lazy;
use pulldown_cmark::{html, CodeBlockKind, Event, Options, Parser, Tag, TagEnd};
use syntect::highlighting::{Theme, ThemeSet};
use syntect::html::highlighted_html_for_string;
use syntect::parsing::SyntaxSet;
use tracing::instrument;

use crate::config::FrontMatter;

static SYNTAX_SET: Lazy<SyntaxSet> = Lazy::new(SyntaxSet::load_defaults_newlines);
static THEME_SET: Lazy<ThemeSet> = Lazy::new(ThemeSet::load_defaults);

pub fn get_syntax_set() -> &'static SyntaxSet {
    &SYNTAX_SET
}

pub fn get_theme(theme_name: &str) -> &'static Theme {
    THEME_SET.themes.get(theme_name).unwrap_or_else(|| {
        tracing::warn!(
            "Theme '{}' not found, falling back to 'base16-ocean.dark'",
            theme_name
        );
        &THEME_SET.themes["base16-ocean.dark"]
    })
}

/// A document that has been fully processed.
#[derive(Debug)]
pub struct ParsedDocument {
    /// Metadata and configuration extracted from YAML front matter.
    pub front_matter: FrontMatter,
    /// The generated HTML fragment from the Markdown body.
    pub html_content: String,
    /// Generated Table of Contents HTML.
    pub toc_html: Option<String>,
}

/// Splits the content into front matter and markdown body.
pub fn parse_front_matter(content: &str) -> anyhow::Result<(FrontMatter, String)> {
    let matter = Matter::<YAML>::new();
    let parsed = matter.parse(content);

    let front_matter: FrontMatter = parsed
        .data
        .map(|fm_data| fm_data.deserialize())
        .transpose()
        .map_err(|e| anyhow::anyhow!("Invalid front matter YAML: {}", e))?
        .unwrap_or_default();

    Ok((front_matter, parsed.content))
}

/// Converts Markdown body to HTML with syntax highlighting and TOC.
#[instrument(skip(content))]
#[instrument(skip(content))]
pub fn generate_html(content: &str, theme_name: &str) -> anyhow::Result<(String, String)> {
    let ss = get_syntax_set();
    let theme = get_theme(theme_name);

    let options = get_markdown_options();
    let parser = Parser::new_ext(content, options);

    let (new_events, headers) = process_markdown_events(content, parser, ss, theme);

    // Generate HTML content
    let mut html_content = String::with_capacity(content.len() * 2);
    html::push_html(&mut html_content, new_events.into_iter());

    // Sanitize HTML (Security P0)
    let sanitized_html = ammonia::Builder::default()
        .add_generic_attributes(&["id", "class", "style", "type", "checked", "disabled"])
        .add_tags(&["span", "input"])
        .clean(&html_content)
        .to_string();

    let toc_html = generate_toc_html(&headers);

    Ok((sanitized_html, toc_html))
}

fn get_markdown_options() -> Options {
    Options::ENABLE_GFM
        | Options::ENABLE_TABLES
        | Options::ENABLE_FOOTNOTES
        | Options::ENABLE_STRIKETHROUGH
        | Options::ENABLE_TASKLISTS
        | Options::ENABLE_SMART_PUNCTUATION
        | Options::ENABLE_HEADING_ATTRIBUTES
}

fn process_markdown_events<'a>(
    _content: &'a str,
    parser: Parser<'a>,
    ss: &'static SyntaxSet,
    theme: &'static syntect::highlighting::Theme,
) -> (Vec<Event<'a>>, Vec<(i32, String, String)>) {
    let mut new_events = Vec::new();
    let mut headers = Vec::new();

    let mut in_code_block = false;
    let mut code_lang = None;
    let mut code_content = String::new();

    let mut in_heading = false;
    let mut current_heading_level = 0;
    let mut current_heading_id = String::new();
    let mut current_heading_text = String::new();

    for event in parser {
        match event {
            Event::Start(Tag::CodeBlock(CodeBlockKind::Fenced(ref lang))) => {
                in_code_block = true;
                code_lang = Some(lang.to_string());
                code_content.clear();
            }
            Event::End(TagEnd::CodeBlock) => {
                in_code_block = false;
                let lang = code_lang.take().unwrap_or_default();
                let syntax = ss
                    .find_syntax_by_token(&lang)
                    .unwrap_or_else(|| ss.find_syntax_plain_text());

                // Perform highlighting (Performance P2 - although spawn_blocking is for async context,
                // generate_html is sync, so we just use it directly. If we were in an async fn,
                // we would use spawn_blocking).
                let highlighted = highlighted_html_for_string(&code_content, ss, syntax, theme)
                    .unwrap_or_else(|_| code_content.clone());

                new_events.push(Event::Html(highlighted.into()));
            }
            Event::Text(ref text) if in_code_block => {
                code_content.push_str(text);
            }
            Event::Start(Tag::Heading { level, ref id, .. }) => {
                in_heading = true;
                current_heading_level = match level {
                    pulldown_cmark::HeadingLevel::H1 => 1,
                    pulldown_cmark::HeadingLevel::H2 => 2,
                    pulldown_cmark::HeadingLevel::H3 => 3,
                    pulldown_cmark::HeadingLevel::H4 => 4,
                    pulldown_cmark::HeadingLevel::H5 => 5,
                    pulldown_cmark::HeadingLevel::H6 => 6,
                };
                current_heading_id = id.clone().unwrap_or_else(|| "".into()).to_string();
                current_heading_text.clear();
                new_events.push(event.clone());
            }
            Event::End(TagEnd::Heading(_)) => {
                in_heading = false;
                if current_heading_id.is_empty() {
                    current_heading_id = slug::slugify(&current_heading_text);
                }
                headers.push((
                    current_heading_level,
                    current_heading_text.clone(),
                    current_heading_id.clone(),
                ));
                new_events.push(event);
            }
            Event::Text(ref text) if in_heading => {
                current_heading_text.push_str(text);
                new_events.push(event);
            }
            _ => {
                if !in_code_block {
                    new_events.push(event);
                }
            }
        }
    }
    (new_events, headers)
}

fn generate_toc_html(headers: &[(i32, String, String)]) -> String {
    if headers.is_empty() {
        return String::new();
    }

    let base_level = headers[0].0;
    let mut toc = String::from("<ul class=\"toc\">");
    let mut last_level = 0;

    for (level, text, id) in headers {
        let level = *level;
        if last_level == 0 {
            last_level = level;
        }

        if level > last_level {
            for _ in 0..(level - last_level) {
                toc.push_str("<ul>");
            }
        } else if level < last_level {
            for _ in 0..(last_level - level) {
                toc.push_str("</ul>");
            }
        }

        use std::fmt::Write;
        let escaped_text = html_escape::encode_text(text);
        let _ = write!(toc, "<li><a href=\"#{}\">{}</a></li>", id, escaped_text);
        last_level = level;
    }

    while last_level > base_level {
        toc.push_str("</ul>");
        last_level -= 1;
    }
    toc.push_str("</ul>");
    toc
}

/// Convenience wrapper that parses everything using front-matter theme or default.
/// Warning: This ignores CLI overrides for theme since it parses internally.
/// Use split functions for full control.
pub fn parse_markdown(content: &str) -> anyhow::Result<ParsedDocument> {
    let (front_matter, raw_md) = parse_front_matter(content)?;
    let theme = front_matter
        .highlight_theme
        .as_deref()
        .unwrap_or("base16-ocean.dark");
    let (html_content, toc_html) = generate_html(&raw_md, theme)?;

    Ok(ParsedDocument {
        front_matter,
        html_content,
        toc_html: Some(toc_html),
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

/// Extracts raw Markdown from a Jupyter Notebook.
pub fn parse_notebook_raw(content: &str) -> anyhow::Result<String> {
    let notebook: Notebook = serde_json::from_str(content)
        .map_err(|e| anyhow::anyhow!("Invalid Jupyter Notebook JSON: {}", e))?;

    let mut markdown = String::with_capacity(content.len());
    for cell in notebook.cells {
        match cell.cell_type.as_str() {
            "markdown" => {
                for line in cell.source {
                    markdown.push_str(&line);
                }
                markdown.push('\n');
            }
            "code" => {
                markdown.push_str("\n```python\n"); // Default to python for notebooks
                for line in cell.source {
                    markdown.push_str(&line);
                }
                markdown.push_str("\n```\n");
            }
            _ => {}
        }
    }
    Ok(markdown)
}

#[instrument(skip(content), fields(content_len = content.len()))]
pub fn parse_notebook(content: &str) -> anyhow::Result<ParsedDocument> {
    let markdown = parse_notebook_raw(content)?;
    // Reuse parse_markdown on the generated string
    parse_markdown(&markdown)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_highlighting() {
        let content = "```rust\nfn main() {}\n```";
        let (html, _) = generate_html(content, "base16-ocean.dark").unwrap();
        assert!(html.contains("<pre style=")); // Syntect adds styles
    }

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

    // ============ Edge Case Tests (TEST-004) ============

    #[test]
    fn test_parse_empty_content() {
        let content = "";
        let doc = parse_markdown(content).unwrap();
        assert!(doc.front_matter.title.is_none());
        assert!(doc.html_content.is_empty() || doc.html_content.trim().is_empty());
    }

    #[test]
    fn test_parse_whitespace_only() {
        let content = "   \n\n   \t   ";
        let doc = parse_markdown(content).unwrap();
        assert!(doc.front_matter.title.is_none());
    }

    #[test]
    fn test_parse_empty_front_matter() {
        let content = "---\n---\n# Content";
        let doc = parse_markdown(content).unwrap();
        assert!(doc.front_matter.title.is_none());
        assert!(doc.html_content.contains("<h1>Content</h1>"));
    }

    #[test]
    fn test_parse_front_matter_empty_values() {
        let content = r#"---
title: ""
author: ""
---
# Content"#;
        let doc = parse_markdown(content).unwrap();
        // Empty strings are still Some("")
        assert_eq!(doc.front_matter.title, Some("".to_string()));
    }

    #[test]
    fn test_parse_front_matter_null_values() {
        let content = r#"---
title: null
author: ~
---
# Content"#;
        let doc = parse_markdown(content).unwrap();
        // YAML null should become None
        assert!(doc.front_matter.title.is_none());
        assert!(doc.front_matter.author.is_none());
    }

    #[test]
    fn test_parse_front_matter_very_long_title() {
        let long_title = "A".repeat(10000);
        let content = format!("---\ntitle: \"{}\"\n---\n# Content", long_title);
        let doc = parse_markdown(&content).unwrap();
        assert_eq!(doc.front_matter.title.as_ref().unwrap().len(), 10000);
    }

    #[test]
    fn test_parse_front_matter_special_characters() {
        let content = r#"---
title: 'Test: A "Quoted" Title (with parens) & symbols'
author: "O'Brien & Sons"
---
# Content"#;
        let doc = parse_markdown(content).unwrap();
        assert!(doc
            .front_matter
            .title
            .as_ref()
            .unwrap()
            .contains("Quoted"));
        assert!(doc
            .front_matter
            .author
            .as_ref()
            .unwrap()
            .contains("O'Brien"));
    }

    #[test]
    fn test_parse_front_matter_unicode() {
        let content = r#"---
title: "Titre en Francais avec des accents"
author: "Jean-Francois Mueller"
---
# Contenu"#;
        let doc = parse_markdown(content).unwrap();
        assert!(doc.front_matter.title.is_some());
        assert!(doc.front_matter.author.is_some());
    }

    #[test]
    fn test_parse_front_matter_multiline_description() {
        let content = r#"---
title: "Test"
description: |
  This is a multiline
  description that spans
  multiple lines.
---
# Content"#;
        let doc = parse_markdown(content).unwrap();
        assert!(doc
            .front_matter
            .description
            .as_ref()
            .unwrap()
            .contains("multiline"));
        assert!(doc
            .front_matter
            .description
            .as_ref()
            .unwrap()
            .contains("spans"));
    }

    #[test]
    fn test_parse_front_matter_keywords_array() {
        let content = r#"---
keywords:
  - rust
  - markdown
  - pdf
---
# Content"#;
        let doc = parse_markdown(content).unwrap();
        let keywords = doc.front_matter.keywords.as_ref().unwrap();
        assert_eq!(keywords.len(), 3);
        assert!(keywords.contains(&"rust".to_string()));
    }

    #[test]
    fn test_parse_very_large_content() {
        let large_content =
            "# Large Document\n\n".to_string() + &"This is a paragraph.\n\n".repeat(1000);
        let doc = parse_markdown(&large_content).unwrap();
        assert!(doc.html_content.len() > 10000);
    }

    #[test]
    fn test_parse_deeply_nested_headers() {
        let content = "# H1\n## H2\n### H3\n#### H4\n##### H5\n###### H6";
        let doc = parse_markdown(content).unwrap();
        assert!(doc.html_content.contains("<h1>"));
        assert!(doc.html_content.contains("<h6>"));
    }

    #[test]
    fn test_notebook_empty_source_lines() {
        let notebook = r#"{
            "cells": [
                {
                    "cell_type": "markdown",
                    "source": []
                },
                {
                    "cell_type": "code",
                    "source": []
                }
            ]
        }"#;
        let doc = parse_notebook(notebook).unwrap();
        // Should succeed with empty content
        assert!(doc.html_content.is_empty() || doc.html_content.contains("<pre"));
    }

    #[test]
    fn test_notebook_unknown_cell_type() {
        let notebook = r##"{
            "cells": [
                {
                    "cell_type": "unknown_type",
                    "source": ["ignored content"]
                },
                {
                    "cell_type": "markdown",
                    "source": ["# Real Content"]
                }
            ]
        }"##;
        let doc = parse_notebook(notebook).unwrap();
        // Unknown cell type should be ignored
        assert!(doc.html_content.contains("<h1>Real Content</h1>"));
        assert!(!doc.html_content.contains("ignored content"));
    }

    #[test]
    fn test_toc_generation() {
        let content = "# Heading 1\n## Heading 2\n### Heading 3";
        let doc = parse_markdown(content).unwrap();
        let toc = doc.toc_html.unwrap();
        assert!(toc.contains("class=\"toc\""));
        assert!(toc.contains("heading-1"));
        assert!(toc.contains("heading-2"));
        assert!(toc.contains("heading-3"));
    }

    #[test]
    fn test_code_block_without_language() {
        let content = "```\nplain code\n```";
        let doc = parse_markdown(content).unwrap();
        assert!(doc.html_content.contains("plain code"));
    }
}
