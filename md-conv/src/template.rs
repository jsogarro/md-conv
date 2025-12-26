use handlebars::Handlebars;
use serde::Serialize;
use std::sync::OnceLock;
use tracing::instrument;

static DEFAULT_TEMPLATE: &str = include_str!("../templates/base.html");
static HANDLEBARS: OnceLock<Handlebars<'static>> = OnceLock::new();

fn get_handlebars() -> &'static Handlebars<'static> {
    HANDLEBARS.get_or_init(|| {
        let mut hb = Handlebars::new();
        hb.set_strict_mode(false); // Allow missing fields
        hb.register_escape_fn(handlebars::no_escape); // We handle escaping ourselves
        hb.register_template_string("base", DEFAULT_TEMPLATE)
            .expect("Failed to register base template");
        hb
    })
}

#[derive(Debug, Serialize)]
pub struct TemplateContext<'a> {
    pub title: Option<&'a str>,
    pub author: Option<&'a str>,
    pub date: Option<&'a str>,
    pub description: Option<&'a str>,
    pub keywords: Option<String>, // Joined with commas
    pub lang: Option<&'a str>,
    pub custom_css: Option<&'a str>,
    pub content: &'a str,
}

/// Render the complete HTML document
#[instrument(skip(ctx), fields(title = ?ctx.title))]
pub fn render_html(ctx: TemplateContext<'_>) -> anyhow::Result<String> {
    let hb = get_handlebars();
    let html = hb.render("base", &ctx)?;
    tracing::debug!(html_len = html.len(), "Rendered HTML template");
    Ok(html)
}

/// Create template context from parsed document and config
pub fn create_context<'a>(
    doc: &'a crate::parser::ParsedDocument,
    config: &'a crate::config::ConversionConfig,
) -> TemplateContext<'a> {
    TemplateContext {
        title: doc.front_matter.title.as_deref(),
        author: doc.front_matter.author.as_deref(),
        date: doc.front_matter.date.as_deref(),
        description: doc.front_matter.description.as_deref(),
        keywords: doc.front_matter.keywords.as_ref().map(|k| k.join(", ")),
        lang: doc.front_matter.lang.as_deref(),
        custom_css: config.css_content.as_deref(),
        content: &doc.html_content,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_render_basic() {
        let ctx = TemplateContext {
            title: Some("Test"),
            author: None,
            date: None,
            description: None,
            keywords: None,
            lang: None,
            custom_css: None,
            content: "<p>Hello</p>",
        };
        let html = render_html(ctx).unwrap();
        assert!(html.contains("<title>Test</title>"));
        assert!(html.contains("<p>Hello</p>"));
        assert!(html.contains("<!DOCTYPE html>"));
    }

    #[test]
    fn test_render_with_custom_css() {
        let ctx = TemplateContext {
            title: None,
            author: None,
            date: None,
            description: None,
            keywords: None,
            lang: None,
            custom_css: Some("body { color: red; }"),
            content: "<p>Test</p>",
        };
        let html = render_html(ctx).unwrap();
        assert!(html.contains("body { color: red; }"));
    }

    #[test]
    fn test_render_with_all_metadata() {
        let ctx = TemplateContext {
            title: Some("My Doc"),
            author: Some("Jane Doe"),
            date: Some("2025-01-01"),
            description: Some("A test document"),
            keywords: Some("test, markdown, pdf".to_string()),
            lang: Some("fr"),
            custom_css: None,
            content: "<p>Bonjour</p>",
        };
        let html = render_html(ctx).unwrap();
        assert!(html.contains("<title>My Doc</title>"));
        assert!(html.contains("By Jane Doe"));
        assert!(html.contains("2025-01-01"));
        assert!(html.contains("lang=\"fr\""));
        assert!(html.contains("test, markdown, pdf"));
    }

    #[test]
    fn test_content_unescaped() {
        let ctx = TemplateContext {
            title: None,
            author: None,
            date: None,
            description: None,
            keywords: None,
            lang: None,
            custom_css: None,
            content: "<script>alert('test')</script>", // Should remain unescaped
        };
        let html = render_html(ctx).unwrap();
        assert!(html.contains("<script>alert('test')</script>"));
    }
}
