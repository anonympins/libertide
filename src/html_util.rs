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
pub fn extract_attribute<'a>(attrs: &'a str, target: &str) -> Option<&'a str> {
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

pub fn push_macro_attr(buf: &mut String, key: &str, val: Option<&str>) {
    if let Some(v) = val {
        let trimmed = v.trim();
        if !trimmed.is_empty() {
            buf.push(' ');
            buf.push_str(key);
            buf.push_str("=\"");
            for c in trimmed.chars() {
                if c == '"' {
                    buf.push('\'');
                } else if c == '\n' || c == '\r' {
                    buf.push(' ');
                } else {
                    buf.push(c);
                }
            }
            buf.push('"');
        }
    }
}

fn get_test_id(tag_body: &str) -> Option<String> {
    extract_attribute(tag_body, "data-testid")
        .or_else(|| extract_attribute(tag_body, "data-test-id"))
        .or_else(|| extract_attribute(tag_body, "data-test"))
        .map(|s| decode_html_entities(s.trim()))
        .filter(|s| !s.is_empty())
}

fn get_attr_decoded(tag_body: &str, attr: &str) -> Option<String> {
    extract_attribute(tag_body, attr)
        .map(|s| decode_html_entities(s.trim()))
        .filter(|s| !s.is_empty())
}

struct ActiveButton {
    index: usize,
    id: Option<String>,
    name: Option<String>,
    test_id: Option<String>,
    aria_label: Option<String>,
    text: String,
}

struct ActiveLink {
    index: usize,
    href: Option<String>,
    id: Option<String>,
    test_id: Option<String>,
    aria_label: Option<String>,
    text: String,
}

struct ActiveTextarea {
    index: usize,
    id: Option<String>,
    name: Option<String>,
    placeholder: Option<String>,
    test_id: Option<String>,
    text: String,
}

struct ActiveSelect {
    index: usize,
    id: Option<String>,
    name: Option<String>,
    test_id: Option<String>,
    options: Vec<String>,
}

struct ActiveMedia {
    index: usize,
    tag: String,
    id: Option<String>,
    src: Option<String>,
    title: Option<String>,
    aria_label: Option<String>,
    test_id: Option<String>,
    text: String,
}

fn flush_media(
    media_opt: &mut Option<ActiveMedia>,
    in_cell: bool,
    current_cell: &mut String,
    output: &mut String,
) {
    if let Some(media) = media_opt.take() {
        let tag_type = if media.tag == "audio" { "audio" } else { "video" };
        let mut macro_str = format!("[@{}:{}", tag_type, media.index);
        push_macro_attr(&mut macro_str, "id", media.id.as_deref());
        push_macro_attr(&mut macro_str, "title", media.title.as_deref());
        push_macro_attr(&mut macro_str, "src", media.src.as_deref());
        let clean_text = media.text.trim();
        let text_val = if !clean_text.is_empty() {
            Some(clean_text)
        } else {
            media.aria_label.as_deref().or(media.title.as_deref()).or(media.id.as_deref())
        };
        push_macro_attr(&mut macro_str, "text", text_val);
        push_macro_attr(&mut macro_str, "aria-label", media.aria_label.as_deref());
        push_macro_attr(&mut macro_str, "data-testid", media.test_id.as_deref());
        macro_str.push(']');

        let target_out = if in_cell { current_cell } else { output };
        if !in_cell && !target_out.is_empty() && !target_out.ends_with('\n') {
            target_out.push('\n');
        }
        target_out.push_str(&macro_str);
        if !in_cell {
            target_out.push('\n');
        }
    }
}

fn flush_button(
    btn_opt: &mut Option<ActiveButton>,
    in_cell: bool,
    current_cell: &mut String,
    output: &mut String,
) {
    if let Some(btn) = btn_opt.take() {
        let mut macro_str = format!("[@btn:{}", btn.index);
        push_macro_attr(&mut macro_str, "id", btn.id.as_deref());
        push_macro_attr(&mut macro_str, "name", btn.name.as_deref());

        let clean_text = btn.text.trim();
        let text_val = if !clean_text.is_empty() {
            Some(clean_text)
        } else {
            btn.aria_label.as_deref().or(btn.name.as_deref()).or(btn.id.as_deref())
        };
        push_macro_attr(&mut macro_str, "text", text_val);
        push_macro_attr(&mut macro_str, "data-testid", btn.test_id.as_deref());
        if text_val.is_none() || text_val != btn.aria_label.as_deref() {
            push_macro_attr(&mut macro_str, "aria-label", btn.aria_label.as_deref());
        }
        macro_str.push(']');

        let target_out = if in_cell { current_cell } else { output };
        if !in_cell && !target_out.is_empty() && !target_out.ends_with('\n') {
            target_out.push(' ');
        }
        target_out.push_str(&macro_str);
        if !in_cell {
            target_out.push(' ');
        }
    }
}

fn flush_link(
    link_opt: &mut Option<ActiveLink>,
    in_cell: bool,
    current_cell: &mut String,
    output: &mut String,
) {
    if let Some(link) = link_opt.take() {
        let clean_text = link.text.trim();
        let href_clean = link.href.as_deref().unwrap_or("").trim();
        let is_empty_anchor = clean_text.is_empty() && link.aria_label.is_none() && (href_clean.is_empty() || href_clean.starts_with('#'));

        if is_empty_anchor {
            return;
        }

        let mut macro_str = format!("[@link:{}", link.index);
        let text_val = if !clean_text.is_empty() {
            Some(clean_text)
        } else {
            link.aria_label.as_deref().or(link.id.as_deref()).or(Some(href_clean))
        };
        push_macro_attr(&mut macro_str, "text", text_val);
        if !href_clean.is_empty() {
            push_macro_attr(&mut macro_str, "href", Some(href_clean));
        }
        push_macro_attr(&mut macro_str, "id", link.id.as_deref());
        push_macro_attr(&mut macro_str, "data-testid", link.test_id.as_deref());
        macro_str.push(']');

        let target_out = if in_cell { current_cell } else { output };
        if !in_cell && !target_out.is_empty() && !target_out.ends_with('\n') {
            target_out.push(' ');
        }
        target_out.push_str(&macro_str);
        if !in_cell {
            target_out.push(' ');
        }
    }
}

fn flush_textarea(
    ta_opt: &mut Option<ActiveTextarea>,
    in_cell: bool,
    current_cell: &mut String,
    output: &mut String,
) {
    if let Some(ta) = ta_opt.take() {
        let mut macro_str = format!("[@textarea:{}", ta.index);
        push_macro_attr(&mut macro_str, "name", ta.name.as_deref());
        push_macro_attr(&mut macro_str, "id", ta.id.as_deref());
        push_macro_attr(&mut macro_str, "placeholder", ta.placeholder.as_deref());
        let val_clean = ta.text.trim();
        if !val_clean.is_empty() {
            push_macro_attr(&mut macro_str, "value", Some(val_clean));
        }
        push_macro_attr(&mut macro_str, "data-testid", ta.test_id.as_deref());
        macro_str.push(']');

        let target_out = if in_cell { current_cell } else { output };
        if !in_cell && !target_out.is_empty() && !target_out.ends_with('\n') {
            target_out.push('\n');
        }
        target_out.push_str(&macro_str);
        if !in_cell {
            target_out.push('\n');
        }
    }
}

fn flush_select(
    sel_opt: &mut Option<ActiveSelect>,
    in_cell: bool,
    current_cell: &mut String,
    output: &mut String,
) {
    if let Some(sel) = sel_opt.take() {
        let mut macro_str = format!("[@select:{}", sel.index);
        push_macro_attr(&mut macro_str, "name", sel.name.as_deref());
        push_macro_attr(&mut macro_str, "id", sel.id.as_deref());
        push_macro_attr(&mut macro_str, "data-testid", sel.test_id.as_deref());
        if !sel.options.is_empty() {
            let opts = sel.options.join(", ");
            push_macro_attr(&mut macro_str, "options", Some(&opts));
        }
        macro_str.push(']');

        let target_out = if in_cell { current_cell } else { output };
        if !in_cell && !target_out.is_empty() && !target_out.ends_with('\n') {
            target_out.push(' ');
        }
        target_out.push_str(&macro_str);
        if !in_cell {
            target_out.push(' ');
        }
    }
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
    let mut elem_counter: usize = 0;

    // Gestion des tables
    let mut in_table = false;
    let mut table_rows: Vec<Vec<String>> = Vec::new();
    let mut current_row: Vec<String> = Vec::new();
    let mut current_cell = String::new();
    let mut in_cell = false;

    // Gestion des macros d'interaction
    let mut active_button: Option<ActiveButton> = None;
    let mut active_link: Option<ActiveLink> = None;
    let mut active_textarea: Option<ActiveTextarea> = None;
    let mut active_select: Option<ActiveSelect> = None;
    let mut active_media: Option<ActiveMedia> = None;
    let mut in_option = false;
    let mut current_option = String::new();

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
                        "input" => {
                            let input_type = extract_attribute(tag_body, "type")
                                .map(|s| s.trim().to_lowercase())
                                .unwrap_or_else(|| "text".to_string());

                            if input_type != "hidden" {
                                elem_counter += 1;
                                let id = get_attr_decoded(tag_body, "id");
                                let name = get_attr_decoded(tag_body, "name");
                                let placeholder = get_attr_decoded(tag_body, "placeholder");
                                let value_attr = extract_attribute(tag_body, "value").map(|s| decode_html_entities(s.trim()));
                                let test_id = get_test_id(tag_body);
                                let aria_label = get_attr_decoded(tag_body, "aria-label");

                                let mut macro_str = String::with_capacity(64);
                                if input_type == "submit" || input_type == "button" || input_type == "reset" {
                                    macro_str.push_str(&format!("[@btn:{elem_counter}"));
                                    push_macro_attr(&mut macro_str, "id", id.as_deref());
                                    push_macro_attr(&mut macro_str, "name", name.as_deref());
                                    let btn_text = value_attr.as_deref()
                                        .or(aria_label.as_deref())
                                        .unwrap_or("Submit");
                                    push_macro_attr(&mut macro_str, "text", Some(btn_text));
                                    push_macro_attr(&mut macro_str, "data-testid", test_id.as_deref());
                                    macro_str.push(']');
                                } else {
                                    macro_str.push_str(&format!("[@input:{elem_counter}"));
                                    if input_type != "text" {
                                        push_macro_attr(&mut macro_str, "type", Some(&input_type));
                                    }
                                    push_macro_attr(&mut macro_str, "name", name.as_deref());
                                    push_macro_attr(&mut macro_str, "id", id.as_deref());
                                    push_macro_attr(&mut macro_str, "placeholder", placeholder.as_deref());

                                    let val_str = value_attr.as_deref().unwrap_or("");
                                    push_macro_attr(&mut macro_str, "value", Some(val_str));

                                    if input_type == "checkbox" || input_type == "radio" {
                                        let is_checked = extract_attribute(tag_body, "checked").is_some()
                                            || tag_body.split_whitespace().any(|w| w == "checked" || w == "checked/>" || w == "checked>");
                                        if is_checked {
                                            push_macro_attr(&mut macro_str, "checked", Some("true"));
                                        }
                                    }

                                    push_macro_attr(&mut macro_str, "data-testid", test_id.as_deref());
                                    push_macro_attr(&mut macro_str, "aria-label", aria_label.as_deref());
                                    macro_str.push(']');
                                }

                                let target_out = if in_cell { &mut current_cell } else { &mut output };
                                if !in_cell && !target_out.is_empty() && !target_out.ends_with('\n') {
                                    target_out.push(' ');
                                }
                                target_out.push_str(&macro_str);
                                if !in_cell {
                                    target_out.push(' ');
                                }
                            }
                        }
                        "textarea" => {
                            if is_closing {
                                flush_textarea(&mut active_textarea, in_cell, &mut current_cell, &mut output);
                            } else {
                                flush_textarea(&mut active_textarea, in_cell, &mut current_cell, &mut output);
                                elem_counter += 1;
                                active_textarea = Some(ActiveTextarea {
                                    index: elem_counter,
                                    id: get_attr_decoded(tag_body, "id"),
                                    name: get_attr_decoded(tag_body, "name"),
                                    placeholder: get_attr_decoded(tag_body, "placeholder"),
                                    test_id: get_test_id(tag_body),
                                    text: String::new(),
                                });
                            }
                        }
                        "select" => {
                            if is_closing {
                                flush_select(&mut active_select, in_cell, &mut current_cell, &mut output);
                            } else {
                                flush_select(&mut active_select, in_cell, &mut current_cell, &mut output);
                                elem_counter += 1;
                                active_select = Some(ActiveSelect {
                                    index: elem_counter,
                                    id: get_attr_decoded(tag_body, "id"),
                                    name: get_attr_decoded(tag_body, "name"),
                                    test_id: get_test_id(tag_body),
                                    options: Vec::new(),
                                });
                            }
                        }
                        "option" => {
                            if is_closing {
                                if let Some(ref mut sel) = active_select {
                                    let opt_text = current_option.trim().to_string();
                                    if !opt_text.is_empty() {
                                        sel.options.push(opt_text);
                                    }
                                }
                                current_option.clear();
                                in_option = false;
                            } else {
                                in_option = true;
                                current_option.clear();
                            }
                        }
                        "video" | "audio" => {
                            if is_closing {
                                flush_media(&mut active_media, in_cell, &mut current_cell, &mut output);
                            } else {
                                flush_media(&mut active_media, in_cell, &mut current_cell, &mut output);
                                elem_counter += 1;
                                active_media = Some(ActiveMedia {
                                    index: elem_counter,
                                    tag: tag_name.to_string(),
                                    id: get_attr_decoded(tag_body, "id"),
                                    src: get_attr_decoded(tag_body, "src"),
                                    title: get_attr_decoded(tag_body, "title"),
                                    aria_label: get_attr_decoded(tag_body, "aria-label"),
                                    test_id: get_test_id(tag_body),
                                    text: String::new(),
                                });
                            }
                        }
                        "source" => {
                            if let Some(ref mut m) = active_media {
                                if m.src.is_none() {
                                    m.src = get_attr_decoded(tag_body, "src");
                                }
                            }
                        }
                        tag if tag == "button" || tag.contains("-button") || tag.ends_with("-btn") => {
                            if is_closing {
                                flush_button(&mut active_button, in_cell, &mut current_cell, &mut output);
                            } else {
                                flush_button(&mut active_button, in_cell, &mut current_cell, &mut output);
                                elem_counter += 1;
                                active_button = Some(ActiveButton {
                                    index: elem_counter,
                                    id: get_attr_decoded(tag_body, "id"),
                                    name: get_attr_decoded(tag_body, "name"),
                                    test_id: get_test_id(tag_body),
                                    aria_label: get_attr_decoded(tag_body, "aria-label"),
                                    text: String::new(),
                                });
                            }
                        }
                        "br" | "hr" => {
                            output.push('\n');
                        }
                        "strong" | "b" => {
                            if active_button.is_none() && active_link.is_none() && active_textarea.is_none() {
                                output.push_str("**");
                            }
                        }
                        "em" | "i" => {
                            if active_button.is_none() && active_link.is_none() && active_textarea.is_none() {
                                output.push('*');
                            }
                        }
                        "code" => {
                            if active_button.is_none() && active_link.is_none() && active_textarea.is_none() {
                                output.push('`');
                            }
                        }
                        "a" => {
                            if is_closing {
                                flush_link(&mut active_link, in_cell, &mut current_cell, &mut output);
                            } else {
                                flush_link(&mut active_link, in_cell, &mut current_cell, &mut output);
                                elem_counter += 1;
                                active_link = Some(ActiveLink {
                                    index: elem_counter,
                                    href: get_attr_decoded(tag_body, "href"),
                                    id: get_attr_decoded(tag_body, "id"),
                                    test_id: get_test_id(tag_body),
                                    aria_label: get_attr_decoded(tag_body, "aria-label"),
                                    text: String::new(),
                                });
                            }
                        }
                        "img" => {
                            if let Some(alt) = extract_attribute(tag_body, "alt") {
                                let trimmed = alt.trim();
                                if !trimmed.is_empty() {
                                    if let Some(ref mut btn) = active_button {
                                        if !btn.text.is_empty() {
                                            btn.text.push(' ');
                                        }
                                        btn.text.push_str(trimmed);
                                    } else if let Some(ref mut link) = active_link {
                                        if !link.text.is_empty() {
                                            link.text.push(' ');
                                        }
                                        link.text.push_str(trimmed);
                                    } else {
                                        output.push_str(&format!(" [Image: {trimmed}] "));
                                    }
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
            if let Some(ref mut ta) = active_textarea {
                ta.text.push(ch);
            } else if let Some(ref mut _sel) = active_select {
                if in_option {
                    current_option.push(ch);
                }
            } else if let Some(ref mut btn) = active_button {
                btn.text.push(ch);
            } else if let Some(ref mut link) = active_link {
                link.text.push(ch);
            } else if let Some(ref mut media) = active_media {
                media.text.push(ch);
            } else if in_cell {
                current_cell.push(ch);
            } else if !in_table {
                output.push(ch);
            }
        }
        i += ch.len_utf8();
    }

    flush_button(&mut active_button, in_cell, &mut current_cell, &mut output);
    flush_link(&mut active_link, in_cell, &mut current_cell, &mut output);
    flush_textarea(&mut active_textarea, in_cell, &mut current_cell, &mut output);
    flush_select(&mut active_select, in_cell, &mut current_cell, &mut output);
    flush_media(&mut active_media, in_cell, &mut current_cell, &mut output);

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