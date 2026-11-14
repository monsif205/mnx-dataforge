//! RFC 4180 CSV reader/writer (quotes, embedded newlines, CRLF, BOM).

pub fn parse(text: &str, delim: char) -> Result<Vec<Vec<String>>, String> {
    let text = text.strip_prefix('\u{feff}').unwrap_or(text);
    let mut rows: Vec<Vec<String>> = Vec::new();
    let mut row: Vec<String> = Vec::new();
    let mut field = String::new();
    let mut in_q = false;
    let mut field_started = false;
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        if in_q {
            if c == '"' {
                if chars.peek() == Some(&'"') {
                    chars.next();
                    field.push('"');
                } else {
                    in_q = false;
                }
            } else {
                field.push(c);
            }
        } else if c == '"' && field.is_empty() && !field_started {
            in_q = true;
            field_started = true;
        } else if c == delim {
            row.push(std::mem::take(&mut field));
            field_started = false;
        } else if c == '\n' || c == '\r' {
            if c == '\r' && chars.peek() == Some(&'\n') {
                chars.next();
            }
            row.push(std::mem::take(&mut field));
            field_started = false;
            rows.push(std::mem::take(&mut row));
        } else {
            field.push(c);
            field_started = true;
        }
    }
    if in_q {
        return Err("unterminated quoted field".to_string());
    }
    if field_started || !field.is_empty() || !row.is_empty() {
        row.push(field);
        rows.push(row);
    }
    Ok(rows)
}

fn needs_quote(f: &str, delim: char) -> bool {
    f.contains(delim) || f.contains('"') || f.contains('\n') || f.contains('\r')
        || f.starts_with(' ') || f.ends_with(' ')
}

pub fn write_row(fields: &[String], delim: char) -> String {
    let mut o = String::new();
    for (i, f) in fields.iter().enumerate() {
        if i > 0 {
            o.push(delim);
        }
        if needs_quote(f, delim) {
            o.push('"');
            o.push_str(&f.replace('"', "\"\""));
            o.push('"');
        } else {
            o.push_str(f);
        }
    }
    o
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrip_tricky() {
        let rows = vec![
            vec!["id".to_string(), "text".to_string()],
            vec!["1".to_string(), "سلام, \"عالم\"\nسطر ثاني".to_string()],
            vec!["2".to_string(), "".to_string()],
        ];
        let mut out = String::new();
        for r in &rows {
            out.push_str(&write_row(r, ','));
            out.push_str("\r\n");
        }
        let back = parse(&out, ',').unwrap();
        assert_eq!(back, rows);
    }

    #[test]
    fn unterminated_quote() {
        assert!(parse("a,\"b", ',').is_err());
    }

    #[test]
    fn bom_and_no_trailing_newline() {
        let r = parse("\u{feff}a,b\n1,2", ',').unwrap();
        assert_eq!(r, vec![vec!["a", "b"], vec!["1", "2"]]);
    }
}
