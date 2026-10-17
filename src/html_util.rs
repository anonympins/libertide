/// Décode les entités HTML courantes ainsi que les entités numériques décimales et hexadécimales.
pub fn decode_html_entities(input: &str) -> String {
    let mut result = String::with_capacity(input.len());
    let mut chars = input.chars().peekable();

    while let Some(c) = chars.next() {
        if c == '&' {
            let mut entity = String::with_capacity(10);
            let mut matched = false;

            while let Some(&next_c) = chars.peek() {
                if next_c == ';' {
                    chars.next(); // consomme le ';'
                    matched = true;
                    break;
                } else if next_c.is_alphanumeric() || next_c == '#' {
                    entity.push(chars.next().unwrap());
                    if entity.len() > 10 {
                        break;
                    }
                } else {
                    break;
                }
            }

            if matched {
                match entity.as_str() {
                    "nbsp" => result.push(' '),
                    "amp" => result.push('&'),
                    "lt" => result.push('<'),
                    "gt" => result.push('>'),
                    "quot" => result.push('"'),
                    "apos" | "#39" => result.push('\''),
                    "laquo" => result.push('«'),
                    "raquo" => result.push('»'),
                    "ndash" => result.push('–'),
                    "mdash" => result.push('—'),
                    "hellip" => result.push('…'),
                    "euro" => result.push('€'),
                    "copy" => result.push('©'),
                    _ if entity.starts_with("#x") || entity.starts_with("#X") => {
                        if let Ok(code) = u32::from_str_radix(&entity[2..], 16) {
                            if let Some(ch) = char::from_u32(code) {
                                result.push(ch);
                            } else {
                                result.push('?');
                            }
                        }
                    }
                    _ if entity.starts_with('#') => {
                        if let Ok(code) = entity[1..].parse::<u32>() {
                            if let Some(ch) = char::from_u32(code) {
                                result.push(ch);
                            } else {
                                result.push('?');
                            }
                        }
                    }
                    _ => {
                        result.push('&');
                        result.push_str(&entity);
                        result.push(';');
                    }
                }
            } else {
                result.push('&');
                result.push_str(&entity);
            }
        } else {
            result.push(c);
        }
    }
    result
}

/// Extrait la valeur d'un attribut (ex: href) dans la chaîne d'attributs d'une balise.
fn extract_attribute<'a>(attrs: &'a str, target: &str) -> Option<&'a str> {
    let target_bytes = target.as_bytes();
    let attrs_bytes = attrs.as_bytes();
    let t_len = target_bytes.len();
    let a_len = attrs_bytes.len();

    if t_len == 0 || a_len < t_len {
        return None;
    }

    let mut i = 0;
    while i + t_len <= a_len {
        let prev_ok = i == 0 || attrs_bytes[i - 1].is_ascii_whitespace() || attrs_bytes[i - 1] == b'/' || attrs_bytes[i - 1] == b'<';
        if prev_ok && attrs_bytes[i..i + t_len].eq_ignore_ascii_case(target_bytes) {
            let mut j = i + t_len;
            while j < a_len && attrs_bytes[j].is_ascii_whitespace() {
                j += 1;
            }
            if j < a_len && attrs_bytes[j] == b'=' {
                j += 1;
                while j < a_len && attrs_bytes[j].is_ascii_whitespace() {
                    j += 1;
                }
                if j < a_len {
                    let quote = attrs_bytes[j];
                    if quote == b'"' || quote == b'\'' {
                        let val_start = j + 1;
                        if let Some(rel_end) = attrs[val_start..].find(quote as char) {
                            return Some(&attrs[val_start..val_start + rel_end]);
                        }
                    } else {
                        let val_start = j;
                        let val_end = attrs[val_start..]
                            .find(|c: char| c.is_whitespace() || c == '>')
                            .map_or(attrs.len(), |pos| val_start + pos);
                        return Some(&attrs[val_start..val_end]);
                    }
                }
            }
        }
        if let Some(ch) = attrs[i..].chars().next() {
            i += ch.len_utf8();
        } else {
            break;
        }
    }
    None
}

/// Indique si une balise est typiquement inline (le texte s'écoule naturellement).
fn is_inline_tag(tag: &str) -> bool {
    matches!(
        tag,
        "span"
            | "a"
            | "strong"
            | "b"
            | "em"
            | "i"
            | "u"
            | "s"
            | "strike"
            | "code"
            | "small"
            | "sub"
            | "sup"
            | "mark"
            | "abbr"
            | "cite"
            | "q"
            | "time"
            | "label"
            | "del"
            | "ins"
            | "kbd"
            | "var"
            | "samp"
            | "bdi"
            | "bdo"
            | "font"
    )
}

/// Indique si une balise est une balise void auto-fermante sans contenu textuel propre.
fn is_void_tag(tag: &str) -> bool {
    matches!(
        tag,
        "area"
            | "base"
            | "br"
            | "col"
            | "embed"
            | "hr"
            | "img"
            | "input"
            | "link"
            | "meta"
            | "param"
            | "source"
            | "track"
            | "wbr"
    )
}

/// Transforme un code HTML brut en texte Markdown épuré et exploitable par un LLM.
pub fn sanitize_html_to_markdown(html: &str) -> String {
    let mut output = String::with_capacity(html.len() / 2);
    let mut skip_stack: Vec<String> = Vec::new();

    // Gestion des tables
    let mut in_table = false;
    let mut table_rows: Vec<Vec<String>> = Vec::new();
    let mut current_row: Vec<String> = Vec::new();
    let mut current_cell = String::new();
    let mut in_cell = false;

    // Gestion des liens
    let mut active_link_url: Option<String> = None;

    let mut i = 0;
    let bytes = html.as_bytes();
    let len = bytes.len();

    while i < len {
        // 1. Élimination des commentaires HTML <!-- ... -->
        if html[i..].starts_with("<!--") {
            if let Some(end_comment) = html[i..].find("-->") {
                i += end_comment + 3;
            } else {
                break;
            }
            continue;
        }

        // 2. Détection d'une balise HTML
        if bytes[i] == b'<' {
            if let Some(tag_end) = html[i..].find('>') {
                let full_tag = &html[i + 1..i + tag_end].trim();
                let is_closing = full_tag.starts_with('/');
                let tag_body = if is_closing {
                    &full_tag[1..].trim()
                } else {
                    full_tag
                };

                let tag_name = tag_body
                    .split_whitespace()
                    .next()
                    .unwrap_or("")
                    .to_lowercase();
                let tag_name = tag_name.trim_end_matches('/');

                // Balises à éliminer intégralement avec leur contenu
                let is_ignored_container = matches!(
                    tag_name,
                    "head" | "script" | "style" | "iframe" | "svg" | "noscript" | "template"
                        | "canvas" | "object" | "embed"
                );

                if is_ignored_container {
                    if is_closing {
                        if let Some(pos) = skip_stack.iter().rposition(|t| t == tag_name) {
                            skip_stack.truncate(pos);
                        }
                    } else if !full_tag.ends_with('/') {
                        skip_stack.push(tag_name.to_string());
                    }
                    i += tag_end + 1;
                    continue;
                }

                if skip_stack.is_empty() {
                    match tag_name {
                        "table" => {
                            if is_closing {
                                in_table = false;
                                if !table_rows.is_empty() {
                                    output.push_str("\n\n");
                                    for (r_idx, row) in table_rows.iter().enumerate() {
                                        output.push_str("| ");
                                        output.push_str(&row.join(" | "));
                                        output.push_str(" |\n");
                                        if r_idx == 0 {
                                            output.push_str("| ");
                                            let sep = row.iter().map(|c| "-".repeat(c.chars().count().max(3))).collect::<Vec<_>>().join(" | ");
                                            output.push_str(&sep);
                                            output.push_str(" |\n");
                                        }
                                    }
                                    output.push('\n');
                                    table_rows.clear();
                                }
                            } else {
                                in_table = true;
                                table_rows.clear();
                                current_row.clear();
                            }
                        }
                        "tr" => {
                            if is_closing {
                                if !current_row.is_empty() {
                                    table_rows.push(std::mem::take(&mut current_row));
                                }
                            } else {
                                current_row.clear();
                            }
                        }
                        "th" | "td" => {
                            if is_closing {
                                in_cell = false;
                                let clean_val = decode_html_entities(current_cell.trim())
                                    .replace('\n', " ")
                                    .replace('|', "\\|");
                                current_row.push(clean_val);
                                current_cell.clear();
                            } else {
                                in_cell = true;
                                current_cell.clear();
                            }
                        }
                        "h1" | "h2" | "h3" | "h4" | "h5" | "h6" => {
                            let level = tag_name.chars().nth(1).and_then(|c| c.to_digit(10)).unwrap_or(2) as usize;
                            if is_closing {
                                output.push_str("\n\n");
                            } else {
                                output.push_str("\n\n");
                                output.push_str(&"#".repeat(level));
                                output.push(' ');
                            }
                        }
                        "li" => {
                            if !is_closing {
                                if !output.is_empty() && !output.ends_with('\n') {
                                    output.push('\n');
                                }
                                output.push_str("- ");
                            } else if !output.ends_with('\n') {
                                output.push('\n');
                            }
                        }
                        "ul" | "ol" => {
                            if is_closing && !output.ends_with('\n') {
                                output.push('\n');
                            }
                        }
                        "pre" => {
                            if is_closing {
                                if !output.ends_with("\n```\n") {
                                    output.push_str("\n```\n");
                                }
                            } else {
                                if !output.is_empty() && !output.ends_with('\n') {
                                    output.push('\n');
                                }
                                output.push_str("```\n");
                            }
                        }
                        "p" | "blockquote" => {
                            if is_closing && !output.ends_with("\n\n") {
                                output.push('\n');
                            }
                        }
                        tag if tag == "button" || tag.contains("-button") || tag.ends_with("-btn") => {
                            if !is_closing {
                                output.push_str("[Bouton:]");
                            } else {
                                output.push_str("] ");
                            }
                        }
                        "br" | "hr" => {
                            output.push('\n');
                        }
                        "strong" | "b" => {
                            output.push_str("**");
                        }
                        "em" | "i" => {
                            output.push('*');
                        }
                        "code" => {
                            output.push('`');
                        }
                        "a" => {
                            if is_closing {
                                if let Some(href) = active_link_url.take() {
                                    output.push_str(&format!("]({href})"));
                                }
                            } else if let Some(href) = extract_attribute(tag_body, "href") {
                                if !href.starts_with('#') && !href.starts_with("javascript:") {
                                    output.push('[');
                                    active_link_url = Some(href.to_string());
                                }
                            }
                        }
                        "img" => {
                            if let Some(alt) = extract_attribute(tag_body, "alt") {
                                let trimmed = alt.trim();
                                if !trimmed.is_empty() {
                                    output.push_str(&format!(" [Image: {trimmed}] "));
                                }
                            }
                        }
                        tag if is_inline_tag(tag) => {
                            // Balise inline (ex: span, small, label, etc.) : continue naturellement
                        }
                        tag if is_void_tag(tag) => {
                            // Balise void sans contenu (ex: meta, link, input) : rien à injecter
                        }
                        _ => {
                            // Tout autre élément (div, header, footer, main, nav, aside, tags customs / composants)
                            if !output.is_empty() && !output.ends_with('\n') {
                                output.push('\n');
                            }
                        }
                    }
                }

                i += tag_end + 1;
                continue;
            }
        }

        // 3. Collecte du texte utile (avancement UTF-8 strict)
        let ch = match html[i..].chars().next() {
            Some(c) => c,
            None => break,
        };
        if skip_stack.is_empty() {
            if in_cell {
                current_cell.push(ch);
            } else if !in_table {
                output.push(ch);
            }
        }
        i += ch.len_utf8();
    }

    let decoded = decode_html_entities(&output);
    let mut cleaned_lines = Vec::new();
    let mut consecutive_empty = 0;
    for line in decoded.lines() {
        let trimmed = line.trim_end();
        if trimmed.is_empty() {
            consecutive_empty += 1;
            if consecutive_empty <= 1 {
                cleaned_lines.push("");
            }
        } else {
            consecutive_empty = 0;
            cleaned_lines.push(trimmed);
        }
    }
    let result = cleaned_lines.join("\n").trim().to_string();
    const MAX_MARKDOWN_BYTES: usize = 15 * 1024; // 15 Ko
    if result.len() > MAX_MARKDOWN_BYTES {
        let truncated: String = result.chars().take(MAX_MARKDOWN_BYTES).collect();
        format!("{truncated}\n[...contenu Markdown tronqué à 15 Ko...]")
    } else {
        result
    }
}