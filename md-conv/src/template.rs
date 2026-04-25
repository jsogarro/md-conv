use handlebars::Handlebars;
use serde::Serialize;
use std::sync::OnceLock;
use tracing::instrument;

static DEFAULT_TEMPLATE: &str = include_str!("../templates/base.html"); // Embedded at compile-time
static HANDLEBARS: OnceLock<Handlebars<'static>> = OnceLock::new();

fn get_handlebars() -> &'static Handlebars<'static> {
    HANDLEBARS.get_or_init(|| {
        let mut hb = Handlebars::new();
        hb.set_strict_mode(false); // Allow missing fields
                                   // Default escaping is enabled - escapes HTML entities in {{double braces}}
                                   // Use {{{triple braces}}} ONLY for pre-sanitized content (markdown HTML output)
        hb.register_template_string("base", DEFAULT_TEMPLATE)
            .expect("Failed to register base template");
        hb
    })
}

/// Data bundle required to populate the Handlebars HTML template.
///
/// Most fields are optional and correspond to document metadata.
/// The `content` field contains the pre-rendered HTML fragment from the
/// Markdown parser.
#[derive(Debug, Serialize)]
pub(crate) struct TemplateContext<'a> {
    /// Document title.
    pub title: Option<&'a str>,
    /// Document author.
    pub author: Option<&'a str>,
    /// Date of publication or creation.
    pub date: Option<&'a str>,
    /// A short summary for metadata.
    pub description: Option<&'a str>,
    /// A comma-separated list of keywords.
    pub keywords: Option<String>,
    /// HTML language attribute (e.g., "en").
    pub lang: Option<&'a str>,
    /// Sanitized CSS rules to be injected into the document head.
    pub custom_css: Option<&'a str>,
    /// Table of Contents HTML.
    pub toc: Option<&'a str>,
    /// The main body content (HTML fragment).
    pub content: &'a str,
}

/// Renders a full HTML document by applying the context to the base template.
///
/// This function uses the `HANDLEBARS` singleton to perform the rendering.
/// It ensures that metadata fields are safely escaped while the main `content`
/// (previously rendered from Markdown) is treated as raw HTML.
///
/// # Errors
/// Returns an error if the Handlebars rendering engine fails.
#[instrument(skip_all)]
pub(crate) fn render_html(ctx: TemplateContext<'_>) -> anyhow::Result<String> {
    let hb = get_handlebars();
    let html = hb.render("base", &ctx)?;
    tracing::debug!(html_len = html.len(), "Rendered HTML template");
    Ok(html)
}

/// Transforms a `ParsedDocument` and `ConversionConfig` into a `TemplateContext`.
pub(crate) fn create_context<'a>(
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
        toc: doc.toc_html.as_deref(),
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
            toc: None,
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
            toc: None,
            content: "<p>Test</p>",
        };
        let html = render_html(ctx).unwrap();
        println!("HTML: {}", html);
        assert!(html.contains("color: red"));
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
            toc: None,
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
        // template::render_html expects PRE-SANITIZED content.
        let raw_content = "<script>alert('test')</script>";
        let sanitized = ammonia::clean(raw_content);

        let ctx = TemplateContext {
            title: None,
            author: None,
            date: None,
            description: None,
            keywords: None,
            lang: None,
            custom_css: None,
            toc: None,
            content: &sanitized,
        };
        let html = render_html(ctx).unwrap();
        // ammonia should have stripped the script tag
        assert!(!html.contains("<script>"));
    }

    #[test]
    fn test_front_matter_xss_escaped() {
        let ctx = TemplateContext {
            title: Some("<script>alert('xss')</script>"),
            author: Some("\" onclick=\"alert('xss')"),
            date: Some("<img src=x onerror=alert('xss')>"),
            description: Some("</title><script>alert('xss')</script>"),
            keywords: Some("<script>".to_string()),
            lang: Some("en\" onload=\"alert('xss')"),
            custom_css: None,
            toc: None,
            content: "<p>Safe content</p>",
        };
        let html = render_html(ctx).unwrap();

        // Verify script tags are escaped in title
        assert!(html.contains("&lt;script&gt;"));
        assert!(!html.contains("<script>alert('xss')</script>"));

        // Verify onclick is escaped in author
        assert!(html.contains("&quot;"));
        assert!(!html.contains("onclick="));

        // Verify content remains unescaped (it's pre-sanitized markdown HTML)
        assert!(html.contains("<p>Safe content</p>"));
    }

    #[test]
    fn test_content_remains_unescaped() {
        // Content uses triple braces and should NOT be escaped
        // because it contains pre-sanitized HTML from markdown rendering
        let ctx = TemplateContext {
            title: None,
            author: None,
            date: None,
            description: None,
            keywords: None,
            lang: None,
            custom_css: None,
            toc: None,
            content: "<h1>Heading</h1><p>Paragraph with <strong>bold</strong></p>",
        };
        let html = render_html(ctx).unwrap();
        assert!(html.contains("<h1>Heading</h1>"));
        assert!(html.contains("<strong>bold</strong>"));
    }

    // ============ Edge Case Tests (TEST-004) ============

    #[test]
    fn test_render_empty_content() {
        let ctx = TemplateContext {
            title: None,
            author: None,
            date: None,
            description: None,
            keywords: None,
            lang: None,
            custom_css: None,
            toc: None,
            content: "",
        };
        let html = render_html(ctx).unwrap();
        assert!(html.contains("<!DOCTYPE html>"));
        assert!(html.contains("<main class=\"content\">"));
    }

    #[test]
    fn test_render_very_large_content() {
        let large_content = "<p>".to_string() + &"x".repeat(100000) + "</p>";
        let ctx = TemplateContext {
            title: None,
            author: None,
            date: None,
            description: None,
            keywords: None,
            lang: None,
            custom_css: None,
            toc: None,
            content: &large_content,
        };
        let html = render_html(ctx).unwrap();
        assert!(html.len() > 100000);
    }

    #[test]
    fn test_render_all_fields_none() {
        let ctx = TemplateContext {
            title: None,
            author: None,
            date: None,
            description: None,
            keywords: None,
            lang: None,
            custom_css: None,
            toc: None,
            content: "<p>Test</p>",
        };
        let html = render_html(ctx).unwrap();
        // Should have default lang
        assert!(html.contains("lang=\"en\""));
    }

    #[test]
    fn test_render_custom_lang() {
        let ctx = TemplateContext {
            title: None,
            author: None,
            date: None,
            description: None,
            keywords: None,
            lang: Some("de"),
            custom_css: None,
            toc: None,
            content: "<p>Guten Tag</p>",
        };
        let html = render_html(ctx).unwrap();
        assert!(html.contains("lang=\"de\""));
    }

    #[test]
    fn test_render_keywords_formatting() {
        let ctx = TemplateContext {
            title: None,
            author: None,
            date: None,
            description: None,
            keywords: Some("rust, markdown, pdf, converter".to_string()),
            lang: None,
            custom_css: None,
            toc: None,
            content: "<p>Test</p>",
        };
        let html = render_html(ctx).unwrap();
        assert!(html.contains("rust, markdown, pdf, converter"));
    }

    #[test]
    fn test_render_css_with_special_chars() {
        let ctx = TemplateContext {
            title: None,
            author: None,
            date: None,
            description: None,
            keywords: None,
            lang: None,
            custom_css: Some("body::before { content: 'Test'; }"),
            toc: None,
            content: "<p>Test</p>",
        };
        let html = render_html(ctx).unwrap();
        assert!(html.contains("body::before"));
    }

    #[test]
    fn test_render_author_without_title() {
        let ctx = TemplateContext {
            title: None,
            author: Some("John Doe"),
            date: None,
            description: None,
            keywords: None,
            lang: None,
            custom_css: None,
            toc: None,
            content: "<p>Test</p>",
        };
        let html = render_html(ctx).unwrap();
        // Author meta tag should still be present
        assert!(html.contains("John Doe") || html.contains("author"));
    }

    #[test]
    fn test_render_date_format_preserved() {
        let ctx = TemplateContext {
            title: Some("Test"),
            author: None,
            date: Some("2025-12-26"),
            description: None,
            keywords: None,
            lang: None,
            custom_css: None,
            toc: None,
            content: "<p>Test</p>",
        };
        let html = render_html(ctx).unwrap();
        assert!(html.contains("2025-12-26"));
    }

    #[test]
    fn test_render_with_toc() {
        let ctx = TemplateContext {
            title: Some("Document"),
            author: None,
            date: None,
            description: None,
            keywords: None,
            lang: None,
            custom_css: None,
            toc: Some("<ul><li><a href=\"#heading\">Heading</a></li></ul>"),
            content: "<h1 id=\"heading\">Heading</h1>",
        };
        let html = render_html(ctx).unwrap();
        assert!(html.contains("<ul><li><a href=\"#heading\">Heading</a></li></ul>"));
    }

    #[test]
    fn test_render_description_in_meta() {
        let ctx = TemplateContext {
            title: Some("Test"),
            author: None,
            date: None,
            description: Some("A test document description"),
            keywords: None,
            lang: None,
            custom_css: None,
            toc: None,
            content: "<p>Test</p>",
        };
        let html = render_html(ctx).unwrap();
        assert!(html.contains("A test document description"));
    }
}
