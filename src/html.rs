//! Teams message bodies are small HTML fragments (`<p>hi</p>`, `<div>..</div>`, mentions,
//! emoji `<img alt=..>`). For plain-text display we keep the text, turn block ends and
//! `<br>` into new lines, use an image's alt text, and decode entities. Tables are kept apart
//! (`with_tables`), as rows of cells, for showing as a grid.

/// Stands in the text where a table goes; the table itself is in the list beside it.
pub const TABLE_MARK: char = '\u{FFFC}';

/// A table of a message: rows of cell texts; the first row is a header when `header`.
#[derive(Debug, Clone, PartialEq)]
pub struct Table {
    pub rows: Vec<Vec<String>>,
    pub header: bool,
}

/// A message quoted in a reply (Teams' `<blockquote itemtype="http://schema.skype.com/Reply">`).
#[derive(Debug, Clone, PartialEq)]
pub struct Quote {
    /// The quoted message's id.
    pub message_id: String,
    pub author: String,
    pub time_ms: Option<i64>,
    /// The start of the quoted message, as Teams sends it.
    pub preview: String,
}

/// The message's HTML without its reply quotes, and the quotes.
pub fn with_quotes(html: &str) -> (String, Vec<Quote>) {
    let lower = html.to_ascii_lowercase();
    let mut out = String::with_capacity(html.len());
    let mut quotes = Vec::new();
    let mut at = 0;
    while let Some(off) = lower[at..].find("<blockquote") {
        let start = at + off;
        let Some(end_off) = lower[start..].find("</blockquote>") else { break };
        let end = start + end_off + "</blockquote>".len();
        let block = &html[start..end];
        let open_end = block.find('>').unwrap_or(0);
        if !block[..open_end].contains("schema.skype.com/Reply") {
            out.push_str(&html[at..end]);
            at = end;
            continue;
        }
        out.push_str(&html[at..start]);
        let inner = |prop: &str| {
            let key = format!("itemprop=\"{prop}\"");
            let i = block.find(&key)?;
            let open = i + block[i..].find('>')? + 1;
            let close = open + block[open..].find("</")?;
            Some((block[..i].rfind('<').map(|t| &block[t..open]).unwrap_or(""), to_plain_text(&block[open..close])))
        };
        let (time_tag, _) = inner("time").unwrap_or_default();
        quotes.push(Quote {
            message_id: attribute(&block[..open_end], "itemid").unwrap_or_default(),
            author: inner("mri").map(|(_, t)| t).unwrap_or_default(),
            time_ms: attribute(time_tag, "itemid").and_then(|v| v.parse().ok()),
            preview: inner("preview").map(|(_, t)| t).unwrap_or_default(),
        });
        at = end;
    }
    out.push_str(&html[at..]);
    (out, quotes)
}

/// The message's text with each table replaced by `TABLE_MARK` on its own line, and the tables.
pub fn with_tables(html: &str) -> (String, Vec<Table>) {
    let lower = html.to_ascii_lowercase();
    if !lower.contains("<table") {
        return (convert(html, true), Vec::new());
    }
    let mut text_html = String::with_capacity(html.len());
    let mut tables = Vec::new();
    let mut at = 0;
    while let Some(off) = lower[at..].find("<table") {
        let start = at + off;
        let Some(end_off) = lower[start..].find("</table>") else { break };
        let end = start + end_off + "</table>".len();
        text_html.push_str(&html[at..start]);
        text_html.push_str(&format!("<p>{TABLE_MARK}</p>"));
        tables.push(parse_table(&html[start..end]));
        at = end;
    }
    text_html.push_str(&html[at..]);
    (convert(&text_html, true), tables)
}

fn parse_table(html: &str) -> Table {
    let lower = html.to_ascii_lowercase();
    let mut rows = Vec::new();
    let mut header = false;
    let mut at = 0;
    while let Some(off) = lower[at..].find("<tr") {
        let start = at + off;
        let end = lower[start..].find("</tr>").map(|e| start + e).unwrap_or(html.len());
        let row_lower = &lower[start..end];
        let mut cells = Vec::new();
        let mut c_at = 0;
        loop {
            let td = row_lower[c_at..].find("<td");
            let th = row_lower[c_at..].find("<th");
            let (off, is_th) = match (td, th) {
                (Some(a), Some(b)) if b < a => (b, true),
                (Some(a), _) => (a, false),
                (None, Some(b)) => (b, true),
                (None, None) => break,
            };
            let cell_start = c_at + off;
            let Some(open_end) = row_lower[cell_start..].find('>').map(|e| cell_start + e + 1) else { break };
            let close = if is_th { "</th>" } else { "</td>" };
            let cell_end = row_lower[open_end..].find(close).map(|e| open_end + e).unwrap_or(row_lower.len());
            if is_th && rows.is_empty() {
                header = true;
            }
            cells.push(to_plain_text(&html[start + open_end..start + cell_end]).replace('\n', " "));
            c_at = (cell_end + close.len()).min(row_lower.len());
        }
        if !cells.is_empty() {
            rows.push(cells);
        }
        at = end.max(start + 3);
    }
    Table { rows, header }
}

pub fn to_plain_text(html: &str) -> String {
    convert(html, false)
}

/// Teams emoji kept as pictures: `EMOJI_START` id `EMOJI_ALT` character `EMOJI_END` stand in
/// the text where each was, for drawing with Teams' own colour picture.
pub const EMOJI_START: char = '\u{E000}';
pub const EMOJI_ALT: char = '\u{E001}';
pub const EMOJI_END: char = '\u{E002}';

/// A piece of a message's text: words, or a Teams emoji (its id and its character).
#[derive(Debug, Clone, PartialEq)]
pub enum Piece<'a> {
    Text(&'a str),
    Emoji { id: &'a str, alt: &'a str },
}

/// Splits text made by `with_tables` into words and emoji.
pub fn pieces(text: &str) -> Vec<Piece<'_>> {
    let mut out = Vec::new();
    let mut rest = text;
    while let Some(start) = rest.find(EMOJI_START) {
        if start > 0 {
            out.push(Piece::Text(&rest[..start]));
        }
        let after = &rest[start + EMOJI_START.len_utf8()..];
        let Some(end) = after.find(EMOJI_END) else {
            rest = after;
            continue;
        };
        let inner = &after[..end];
        let (id, alt) = inner.split_once(EMOJI_ALT).unwrap_or((inner, ""));
        out.push(Piece::Emoji { id, alt });
        rest = &after[end + EMOJI_END.len_utf8()..];
    }
    if !rest.is_empty() {
        out.push(Piece::Text(rest));
    }
    out
}

/// A one-line preview's text with Teams emoji kept as pictures (see `pieces`).
pub fn preview_text(html: &str) -> String {
    convert(html, true)
}

/// The text as plain characters: emoji as themselves, tables as "[table]" (for previews,
/// notifications and editing).
pub fn plain(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for p in pieces(text) {
        match p {
            Piece::Text(t) => out.push_str(t),
            Piece::Emoji { alt, .. } => out.push_str(alt),
        }
    }
    out.replace(TABLE_MARK, "[table]")
}

fn convert(html: &str, rich: bool) -> String {
    let mut out = String::with_capacity(html.len());
    let mut chars = html.char_indices().peekable();
    while let Some((i, c)) = chars.next() {
        match c {
            '<' => {
                let rest = &html[i..];
                // An unclosed tag runs to the end; `end` stays on a char boundary either way.
                let end = rest.find('>').map(|e| i + e).unwrap_or(html.len());
                let tag = &html[i + 1..end];
                handle_tag(tag, &mut out, rich);
                while let Some(&(j, _)) = chars.peek() {
                    if j > end {
                        break;
                    }
                    chars.next();
                }
            }
            '&' => {
                // Entities are short ASCII names; look for the ';' among the next few chars,
                // never slicing by byte count, as text after '&' may be multi-byte (č, ě...).
                let rest = &html[i..];
                let semi = rest
                    .char_indices()
                    .take(10)
                    .find(|&(_, ch)| ch == ';')
                    .map(|(k, _)| k);
                if let Some(semi) = semi {
                    if let Some(decoded) = decode_entity(&rest[1..semi]) {
                        out.push(decoded);
                        while chars.peek().is_some_and(|&(j, _)| j <= i + semi) {
                            chars.next();
                        }
                        continue;
                    }
                }
                out.push('&');
            }
            '\r' | '\n' => {
                if !out.ends_with(' ') && !out.ends_with('\n') && !out.is_empty() {
                    out.push(' ');
                }
            }
            _ => out.push(c),
        }
    }
    let collapsed: Vec<&str> = out.lines().map(|l| l.trim_matches([' ', '\t'])).collect();
    // A line of nothing but non-breaking spaces is an empty line.
    let collapsed: Vec<&str> = collapsed.into_iter().map(|l| if l.chars().all(|c| c == '\u{A0}') { "" } else { l }).collect();
    let mut text = collapsed.join("\n");
    while text.contains("\n\n\n") {
        text = text.replace("\n\n\n", "\n\n");
    }
    // Emoji variation selectors have no glyph in our fonts and would show as boxes.
    text.retain(|c| c != '\u{FE0F}' && c != '\u{FE0E}');
    text.trim_matches(|c: char| c.is_whitespace() && c != '\u{A0}').to_string()
}

fn handle_tag(tag: &str, out: &mut String, rich: bool) {
    let lower = tag.trim().to_ascii_lowercase();
    let name: String = lower
        .trim_start_matches('/')
        .chars()
        .take_while(|c| c.is_ascii_alphanumeric())
        .collect();
    let closing = lower.starts_with('/');
    match name.as_str() {
        "br" => out.push('\n'),
        "p" | "div" | "li" | "tr" | "h1" | "h2" | "h3" | "blockquote" if closing => {
            if !out.ends_with('\n') {
                out.push('\n');
            }
        }
        "li" if !closing => out.push_str("• "),
        "img" => {
            let emoji_id = attribute(tag, "itemid").filter(|_| rich && tag.contains("schema.skype.com/Emoji"));
            match (emoji_id, attribute(tag, "alt")) {
                (Some(id), alt) if id.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_') => {
                    out.push(EMOJI_START);
                    out.push_str(&id);
                    out.push(EMOJI_ALT);
                    out.push_str(&alt.unwrap_or_default());
                    out.push(EMOJI_END);
                }
                (_, Some(alt)) => out.push_str(&alt),
                _ => {}
            }
        }
        _ => {}
    }
}

fn attribute(tag: &str, name: &str) -> Option<String> {
    let lower = tag.to_ascii_lowercase();
    let start = lower.find(&format!("{name}=\""))? + name.len() + 2;
    let len = tag[start..].find('"')?;
    Some(tag[start..start + len].to_string())
}

fn decode_entity(entity: &str) -> Option<char> {
    match entity {
        "amp" => Some('&'),
        "lt" => Some('<'),
        "gt" => Some('>'),
        "quot" => Some('"'),
        "apos" => Some('\''),
        // Kept as a non-breaking space: lines indented with them (text trees) stay indented.
        "nbsp" => Some('\u{A0}'),
        _ => {
            let num = entity.strip_prefix('#')?;
            let code = if let Some(hex) = num.strip_prefix(['x', 'X']) {
                u32::from_str_radix(hex, 16).ok()?
            } else {
                num.parse().ok()?
            };
            char::from_u32(code)
        }
    }
}

/// The outgoing side: plain text typed by the person, as the HTML body Teams sends.
pub fn from_plain_text(text: &str) -> String {
    let escaped = text
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;");
    format!("<p>{}</p>", escaped.replace('\n', "<br>"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reply_quotes_come_apart() {
        let html = r#"<blockquote itemscope itemtype="http://schema.skype.com/Reply" itemid="1795610337167"><strong itemprop="mri" itemid="8:orgid:b">Zuzana V</strong><span itemprop="time" itemid="1795610337167"></span><p itemprop="preview">Hele a tohle jsi nekde resil?</p></blockquote>
<p>Aha, ja uz asi vim</p>"#;
        let (rest, quotes) = with_quotes(html);
        assert_eq!(quotes, vec![Quote { message_id: "1795610337167".into(), author: "Zuzana V".into(), time_ms: Some(1795610337167), preview: "Hele a tohle jsi nekde resil?".into() }]);
        assert_eq!(to_plain_text(&rest), "Aha, ja uz asi vim");
        let plain = "<blockquote>just a quote</blockquote>";
        assert_eq!(with_quotes(plain), (plain.to_string(), Vec::new()), "other blockquotes stay");
    }

    #[test]
    fn teams_emoji_stay_pictures_in_messages() {
        let html = r#"<p>benche&nbsp;<span><img itemtype="http://schema.skype.com/Emoji" itemid="grinningfacewithsmilingeyes" src="x" alt="😄"></span> ok</p>"#;
        let (text, _) = with_tables(html);
        let p = pieces(&text);
        assert_eq!(p[0], Piece::Text("benche\u{A0}"));
        assert_eq!(p[1], Piece::Emoji { id: "grinningfacewithsmilingeyes", alt: "😄" });
        assert_eq!(plain(&text), "benche\u{A0}😄 ok");
        assert_eq!(to_plain_text(html), "benche\u{A0}😄 ok", "previews keep the character");
    }

    #[test]
    fn tables_come_apart_and_trees_keep_their_indent() {
        let html = "<p>raptor/<br>\n│&nbsp;&nbsp; ├── raptor-parent</p><table><thead><tr><th>Name</th><th>URL</th></tr></thead><tbody><tr><td>a</td><td><a href=\"x\"><code>https://x</code></a></td></tr></tbody></table><p>after</p>";
        let (text, tables) = with_tables(html);
        assert_eq!(tables.len(), 1);
        assert!(tables[0].header);
        assert_eq!(tables[0].rows, vec![vec!["Name".to_string(), "URL".into()], vec!["a".into(), "https://x".into()]]);
        assert!(text.contains("│\u{A0}\u{A0} ├── raptor-parent"), "{text:?}");
        assert!(text.contains(TABLE_MARK) && text.ends_with("after"));
        assert_eq!(to_plain_text("<p>&nbsp;&nbsp;indented</p>"), "\u{A0}\u{A0}indented");
    }

    #[test]
    fn paragraphs_and_entities() {
        assert_eq!(to_plain_text("<p>a &amp; b</p><p>c</p>"), "a & b\nc");
        assert_eq!(to_plain_text("<div>x<br>y</div>"), "x\ny");
        assert_eq!(to_plain_text("<p>hi <img alt=\"😀\" src=\"x\"></p>"), "hi 😀");
    }

    #[test]
    fn multibyte_text_near_entities_and_tags() {
        assert_eq!(
            to_plain_text("<p>&nbsp; dvěma &quot;x&quot;</p>"),
            "\u{A0} dvěma \"x\""
        );
        assert_eq!(to_plain_text("a & čěšř ž;"), "a & čěšř ž;");
        assert_eq!(to_plain_text("ok <b"), "ok");
        assert_eq!(to_plain_text("ž<"), "ž");
    }

    #[test]
    fn round_trip() {
        assert_eq!(to_plain_text(&from_plain_text("1 < 2\nok")), "1 < 2\nok");
    }
}
