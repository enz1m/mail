//! Identify only consented image resources; stylesheets and fonts stay offline.

use cssparser::{Delimiter, Parser, Token};
use html5ever::tokenizer::{
    BufferQueue, TagKind, Token as HtmlToken, TokenSink, TokenSinkResult, Tokenizer,
    states::RawKind,
};
use std::cell::{Cell, RefCell};
use std::collections::HashSet;

#[derive(Default)]
struct Styles {
    in_style: Cell<bool>,
    text: RefCell<String>,
    rules: RefCell<Vec<String>>,
}
impl TokenSink for Styles {
    type Handle = ();
    fn process_token(&self, token: HtmlToken, _: u64) -> TokenSinkResult<()> {
        match token {
            HtmlToken::TagToken(tag) => {
                for attr in tag
                    .attrs
                    .iter()
                    .filter(|a| a.name.local == html5ever::local_name!("style"))
                {
                    self.rules.borrow_mut().push(attr.value.to_string());
                }
                if tag.name == html5ever::local_name!("style") {
                    if tag.kind == TagKind::StartTag {
                        self.in_style.set(true);
                        return TokenSinkResult::RawData(RawKind::Rawtext);
                    }
                    self.in_style.set(false);
                    self.rules
                        .borrow_mut()
                        .push(std::mem::take(&mut *self.text.borrow_mut()));
                }
            }
            HtmlToken::CharacterTokens(text) if self.in_style.get() => {
                self.text.borrow_mut().push_str(&text);
            }
            _ => {}
        }
        TokenSinkResult::Continue
    }
}

pub(super) fn background_urls(html: &str) -> HashSet<String> {
    let input = BufferQueue::default();
    input.push_back(html.into());
    let tokenizer = Tokenizer::new(Styles::default(), Default::default());
    let _ = tokenizer.feed(&input);
    tokenizer.end();
    let mut urls = HashSet::new();
    for css in tokenizer.sink.rules.into_inner() {
        rules(&mut Parser::new(&css), &mut urls);
    }
    urls
}
fn rules(parser: &mut Parser<'_>, urls: &mut HashSet<String>) {
    while let Ok(token) = parser.next().cloned() {
        match token {
            Token::CurlyBracketBlock => {
                let _ = parser.parse_nested_block(|p| {
                    rules(p, urls);
                    Ok::<_, cssparser::ParseError<()>>(())
                });
            }
            Token::Ident(name)
                if (name.eq_ignore_ascii_case("background")
                    || name.eq_ignore_ascii_case("background-image"))
                    && parser.try_parse(Parser::expect_colon).is_ok() =>
            {
                let _ = parser.parse_until_before(Delimiter::Semicolon, |p| {
                    values(p, urls);
                    Ok::<_, cssparser::ParseError<()>>(())
                });
            }
            _ => {}
        }
    }
}
fn values(parser: &mut Parser<'_>, urls: &mut HashSet<String>) {
    while !parser.is_exhausted() {
        if let Ok(value) = parser.try_parse(Parser::expect_url) {
            if let Ok(url) = url::Url::parse(&value)
                && url.scheme() == "https"
                && urls.len() < 256
            {
                urls.insert(url.to_string());
            }
        } else if let Ok(Token::Function(_)) = parser.next().cloned() {
            let _ = parser.parse_nested_block(|p| {
                values(p, urls);
                Ok::<_, cssparser::ParseError<()>>(())
            });
        }
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn image_consent_does_not_authorize_stylesheet_or_font_fetches() {
        let source = "<style>@import url(https://example.invalid/css);@font-face{font-family:remote;src:url(https://example.invalid/font)} @media(max-width:500px){.hero{background-image:url('https://example.invalid/photo.png')}}</style><div style='background:url(https://example.invalid/other.png)'>Message</div>";
        let (_, blocked) = super::super::approved_document(source, false);
        assert!(blocked.is_empty());
        let (_, allowed) = super::super::approved_document(source, true);
        assert_eq!(
            allowed,
            std::collections::HashSet::from([
                "https://example.invalid/photo.png".to_owned(),
                "https://example.invalid/other.png".to_owned()
            ])
        );
    }
}
