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
    let mut idx = 0;
    let lower = attrs.to_lowercase();
    while let Some(pos) = lower[idx..].find(target) {
        let start = idx + pos + target.len();
        let mut after = attrs[start..].trim_start();
        if after.starts_with('=') {
            after = after[1..].trim_start();
            if let Some(quote) = after.chars().next() {
                if quote == '"' || quote == '\'' {
                    let content = &after[1..];
                    if let Some(end_quote) = content.find(quote) {
                        return Some(&content[..end_quote]);
                    }
                } else {
                    let end = after.find(|c: char| c.is_whitespace() || c == '>').unwrap_or(after.len());
                    return Some(&after[..end]);
                }
            }
        }
        idx = start;
    }
    None
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
                        | "canvas" | "object" | "embed" | "meta" | "link"
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
                                output.push_str("\n- ");
                            }
                        }
                        "p" | "div" | "article" | "section" | "blockquote" => {
                            if is_closing && !output.ends_with("\n\n") {
                                output.push('\n');
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
                        _ => {}
                    }
                }

                i += tag_end + 1;
                continue;
            }
        }

        // 3. Collecte du texte utile
        if skip_stack.is_empty() {
            let ch = html[i..].chars().next().unwrap();
            if in_cell {
                current_cell.push(ch);
            } else if !in_table {
                output.push(ch);
            }
            i += ch.len_utf8();
        } else {
            i += 1;
        }
    }

    let decoded = decode_html_entities(&output);
    decoded.lines().map(|l| l.trim_end()).collect::<Vec<&str>>().join("\n").trim().to_string()
}