//! Bounded local mail search with quoted phrases and field operators.
//! Search evaluates message metadata and local text without network access.

use super::types::{MailboxStore, Message};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum Field {
    Any,
    From,
    To,
    Subject,
    Body,
}

// One parsed term. Every term in a query must match (AND).
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum Term {
    Text { field: Field, needle: String },
    Unread(bool),
    Flagged,
    HasAttachment,
    OnOrAfter(i64),
    Before(i64),
    NewerThan(i64),
    OlderThan(i64),
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub(crate) struct SearchQuery {
    terms: Vec<Term>,
}

impl SearchQuery {
    // True when the query constrains nothing (blank, or only whitespace and
    // stray quotes). Callers use this to skip filtering entirely.
    pub(crate) fn is_empty(&self) -> bool {
        self.terms.is_empty()
    }

    // IMAP SEARCH returns a superset when local predicates cannot be expressed remotely.
    pub(crate) fn imap_criteria(&self, today: super::types::MessageDate) -> Option<String> {
        let today = day_number(today.year, today.month, today.day);
        let keys: Vec<String> = self
            .terms
            .iter()
            .filter_map(|term| match term {
                Term::Text { field, needle } => {
                    let key = match field {
                        Field::Any => "TEXT",
                        Field::From => "FROM",
                        Field::To => "TO",
                        Field::Subject => "SUBJECT",
                        Field::Body => "BODY",
                    };
                    imap_quoted(needle).map(|quoted| format!("{key} {quoted}"))
                }
                Term::Unread(true) => Some("UNSEEN".to_owned()),
                Term::Unread(false) => Some("SEEN".to_owned()),
                Term::Flagged => Some("FLAGGED".to_owned()),
                Term::HasAttachment => None,
                Term::OnOrAfter(day) => Some(format!("SINCE {}", imap_date(*day))),
                Term::Before(day) => Some(format!("BEFORE {}", imap_date(*day))),
                Term::NewerThan(days) => Some(format!("SINCE {}", imap_date(today - days))),
                Term::OlderThan(days) => Some(format!("BEFORE {}", imap_date(today - days))),
            })
            .collect();
        (!keys.is_empty()).then(|| keys.join(" "))
    }

    // Whether this message satisfies every term; `store` supplies only the
    // memoised body text.
    pub(crate) fn matches(&self, store: &MailboxStore, m: &Message) -> bool {
        self.terms.iter().all(|t| term_matches(t, store, m))
    }
}

fn term_matches(term: &Term, store: &MailboxStore, m: &Message) -> bool {
    let day = || day_number(m.date.year, m.date.month, m.date.day);
    let today = || day_number(store.today.year, store.today.month, store.today.day);
    match term {
        Term::OnOrAfter(threshold) => day() >= *threshold,
        Term::Before(threshold) => day() < *threshold,
        Term::NewerThan(days) => today() - day() <= *days,
        Term::OlderThan(days) => today() - day() > *days,
        Term::Unread(want) => m.unread == *want,
        Term::Flagged => m.flagged,
        Term::HasAttachment => m.has_attachment || !m.attachments.is_empty(),
        Term::Text { field, needle } => match field {
            Field::Subject => contains_ci(&m.subject, needle),
            Field::From => {
                contains_ci(&m.sender_name, needle) || contains_ci(&m.sender_email, needle)
            }
            Field::To => m.to.iter().any(|a| contains_ci(a, needle)),
            Field::Body => store.body_matches_query(m, needle),
            Field::Any => {
                contains_ci(&m.subject, needle)
                    || contains_ci(&m.sender_name, needle)
                    || contains_ci(&m.sender_email, needle)
                    || contains_ci(&m.preview, needle)
                    || store.body_matches_query(m, needle)
            }
        },
    }
}

// Days since 1970-01-01 for a civil date (proleptic Gregorian), so dates
// compare and subtract as integers.
pub(crate) fn day_number(year: u32, month: u32, day: u32) -> i64 {
    let (year, month, day) = (
        i64::from(year),
        i64::from(month.clamp(1, 12)),
        i64::from(day.max(1)),
    );
    let year = if month <= 2 { year - 1 } else { year };
    let era = year.div_euclid(400);
    let year_of_era = year - era * 400;
    let month_index = (month + 9) % 12;
    let day_of_year = (153 * month_index + 2) / 5 + day - 1;
    let day_of_era = year_of_era * 365 + year_of_era / 4 - year_of_era / 100 + day_of_year;
    era * 146_097 + day_of_era - 719_468
}

pub(crate) fn civil_date(days: i64) -> (i64, u32, u32) {
    let days = days + 719_468;
    let era = days.div_euclid(146_097);
    let day_of_era = days - era * 146_097;
    let year_of_era =
        (day_of_era - day_of_era / 1460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let month_index = (5 * day_of_year + 2) / 153;
    let day = (day_of_year - (153 * month_index + 2) / 5 + 1) as u32;
    let month = if month_index < 10 {
        month_index + 3
    } else {
        month_index - 9
    } as u32;
    let year = year_of_era + era * 400 + i64::from(month <= 2);
    (year, month, day)
}

// An IMAP `date` (RFC 3501): `1-Mar-2025`.
fn imap_date(days: i64) -> String {
    const MONTHS: [&str; 12] = [
        "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
    ];
    let (year, month, day) = civil_date(days);
    format!("{day}-{}-{year}", MONTHS[(month - 1) as usize])
}

// An IMAP quoted string, or `None` for text a quoted string cannot carry.
fn imap_quoted(text: &str) -> Option<String> {
    if text.is_empty() || !text.chars().all(|c| c.is_ascii() && !c.is_ascii_control()) {
        return None;
    }
    Some(format!(
        "\"{}\"",
        text.replace('\\', "\\\\").replace('"', "\\\"")
    ))
}

fn parse_day(value: &str) -> Option<i64> {
    let mut parts = value.split(['-', '/']);
    let year: u32 = parts
        .next()?
        .parse()
        .ok()
        .filter(|y| (1900..=9999).contains(y))?;
    let month: u32 = match parts.next() {
        Some(month) => month.parse().ok().filter(|m| (1..=12).contains(m))?,
        None => 1,
    };
    let day: u32 = match parts.next() {
        Some(day) => day.parse().ok().filter(|d| (1..=31).contains(d))?,
        None => 1,
    };
    if parts.next().is_some() {
        return None;
    }
    Some(day_number(year, month, day))
}

fn parse_span(value: &str) -> Option<i64> {
    let unit = value.chars().last()?;
    let count: i64 = value.get(..value.len() - unit.len_utf8())?.parse().ok()?;
    let days = match unit {
        'd' => 1,
        'w' => 7,
        'm' => 30,
        'y' => 365,
        _ => return None,
    };
    (count >= 0).then(|| count * days)
}

fn contains_ci(haystack: &str, needle_lower: &str) -> bool {
    haystack.to_ascii_lowercase().contains(needle_lower)
}

fn tokenize(raw: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    let mut cur = String::new();
    let mut in_quotes = false;
    for ch in raw.chars() {
        match ch {
            '"' => in_quotes = !in_quotes,
            c if c.is_whitespace() && !in_quotes => {
                if !cur.trim().is_empty() {
                    out.push(std::mem::take(&mut cur));
                } else {
                    cur.clear();
                }
            }
            c => cur.push(c),
        }
    }
    if !cur.trim().is_empty() {
        out.push(cur);
    }
    out
}

// Parse a raw query string. Never fails: anything unrecognised degrades to a
// literal text term, so the field can never reject what the user typed.
pub(crate) fn parse(raw: &str) -> SearchQuery {
    let mut terms: Vec<Term> = Vec::new();
    for token in tokenize(raw) {
        let (key, value) = match token.split_once(':') {
            Some((k, v)) => (k.to_ascii_lowercase(), v.trim().to_owned()),
            None => (String::new(), String::new()),
        };
        let value_lower = value.to_ascii_lowercase();
        let term = match (key.as_str(), value_lower.as_str()) {
            ("is", "unread") => Some(Term::Unread(true)),
            ("is", "read") => Some(Term::Unread(false)),
            ("is", "flagged" | "starred") => Some(Term::Flagged),
            ("has", "attachment" | "attachments") => Some(Term::HasAttachment),
            ("after", v) => parse_day(v).map(Term::OnOrAfter),
            ("before", v) => parse_day(v).map(Term::Before),
            ("newer_than", v) => parse_span(v).map(Term::NewerThan),
            ("older_than", v) => parse_span(v).map(Term::OlderThan),
            ("from", v) if !v.is_empty() => Some(Term::Text {
                field: Field::From,
                needle: v.to_owned(),
            }),
            ("to", v) if !v.is_empty() => Some(Term::Text {
                field: Field::To,
                needle: v.to_owned(),
            }),
            ("subject", v) if !v.is_empty() => Some(Term::Text {
                field: Field::Subject,
                needle: v.to_owned(),
            }),
            ("body", v) if !v.is_empty() => Some(Term::Text {
                field: Field::Body,
                needle: v.to_owned(),
            }),
            _ => None,
        };
        match term {
            Some(t) => terms.push(t),
            None => {
                let needle = token.trim().to_ascii_lowercase();
                if !needle.is_empty() {
                    terms.push(Term::Text {
                        field: Field::Any,
                        needle,
                    });
                }
            }
        }
    }
    SearchQuery { terms }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_query_becomes_imap_search_keys() {
        let today = super::super::types::MessageDate {
            year: 2025,
            month: 3,
            day: 24,
            minute: 0,
        };
        assert_eq!(
            parse("from:priya \"retro notes\" after:2025-03-01 before:2025/03/20 is:unread")
                .imap_criteria(today)
                .as_deref(),
            Some("FROM \"priya\" TEXT \"retro notes\" SINCE 1-Mar-2025 BEFORE 20-Mar-2025 UNSEEN")
        );
        assert_eq!(
            parse("newer_than:1w").imap_criteria(today).as_deref(),
            Some("SINCE 17-Mar-2025")
        );
        assert_eq!(parse("has:attachment").imap_criteria(today), None);
        assert_eq!(
            parse("café lunch").imap_criteria(today).as_deref(),
            Some("TEXT \"lunch\"")
        );
        assert_eq!(
            parse(r#"say"hi"#).imap_criteria(today).as_deref(),
            Some(r#"TEXT "sayhi""#)
        );
        assert_eq!(civil_date(day_number(2024, 2, 29)), (2024, 2, 29));
    }
}
