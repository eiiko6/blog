use pulldown_cmark::{CodeBlockKind, CowStr, Event, Parser as MarkdownParser, Tag, TagEnd};
use pulldown_cmark_escape::escape_html;
use syntect::html::highlighted_html_for_string;

use crate::{SYNTAX_SET, THEME_SET};

// I found this at <https://github.com/pulldown-cmark/pulldown-cmark/issues/167#issuecomment-3700787117>

pub struct CodeblockRenderer<'a> {
    inner: MarkdownParser<'a>,
}

impl<'a> CodeblockRenderer<'a> {
    pub fn new(inner: MarkdownParser<'a>) -> Self {
        Self { inner }
    }
}

impl<'a> Iterator for CodeblockRenderer<'a> {
    type Item = Event<'a>;

    fn next(&mut self) -> Option<Self::Item> {
        let event = self.inner.next()?;

        // Intercept CodeBlock starts
        let Event::Start(Tag::CodeBlock(kind)) = event else {
            return Some(event);
        };

        let mut code_content = String::new();

        while let Some(inner_event) = self.inner.next() {
            match inner_event {
                Event::End(TagEnd::CodeBlock) => break,
                Event::Text(code) => code_content.push_str(&code),
                _ => {}
            }
        }

        let lang = match kind {
            CodeBlockKind::Indented => "text",
            CodeBlockKind::Fenced(ref language) => language.as_ref(),
        };

        let rendered_html = render_code_to_html(&code_content, lang);

        let mut escaped_code = String::new();
        let _ = escape_html(&mut escaped_code, &code_content);

        let rendered_html =
            rendered_html.replace("<pre", &format!("<pre data-code=\"{}\"", escaped_code));

        Some(Event::Html(CowStr::Boxed(rendered_html.into_boxed_str())))
    }
}

pub fn render_code_to_html(code: &str, lang: &str) -> String {
    let syntax = SYNTAX_SET
        .find_syntax_by_token(lang)
        .unwrap_or_else(|| SYNTAX_SET.find_syntax_plain_text());

    let theme = &THEME_SET.themes["Catppuccin Macchiato"];

    highlighted_html_for_string(code, &SYNTAX_SET, syntax, theme)
        .unwrap_or_else(|_| format!("<pre><code>{}</code></pre>", code))
}
