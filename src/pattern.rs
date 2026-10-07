#[derive(Clone, Debug)]
enum CharClass {
    Any,
    Literal(char),
    Digit,
    NotDigit,
    Word,
    NotWord,
    Whitespace,
    NotWhitespace,
    Custom {
        chars: Vec<char>,
        ranges: Vec<(char, char)>,
        negated: bool,
    },
}

impl CharClass {
    fn matches(&self, c: char) -> bool {
        let cl = c.to_ascii_lowercase();
        match self {
            CharClass::Any => c != '\n' && c != '\r',
            CharClass::Literal(lit) => cl == *lit,
            CharClass::Digit => c.is_ascii_digit(),
            CharClass::NotDigit => !c.is_ascii_digit(),
            CharClass::Word => c.is_alphanumeric() || c == '_',
            CharClass::NotWord => !(c.is_alphanumeric() || c == '_'),
            CharClass::Whitespace => c.is_whitespace(),
            CharClass::NotWhitespace => !c.is_whitespace(),
            CharClass::Custom { chars, ranges, negated } => {
                let hit = chars.contains(&cl)
                    || ranges.iter().any(|&(start, end)| cl >= start && cl <= end);
                if *negated { !hit } else { hit }
            }
        }
    }
}

#[derive(Clone, Copy, Debug)]
enum Quantifier {
    Once,
    ZeroOrMore,
    OneOrMore,
    ZeroOrOne,
}

#[derive(Clone, Debug)]
enum AtomKind {
    Char(CharClass),
    AnchorStart,
    AnchorEnd,
}

#[derive(Clone, Debug)]
struct PatternAtom {
    kind: AtomKind,
    quant: Quantifier,
}

pub fn clean_words(text: &str) -> Vec<String> {
    text.to_lowercase()
        .chars()
        .map(|c| if c.is_alphanumeric() { c } else { ' ' })
        .collect::<String>()
        .split_whitespace()
        .filter(|w| !w.is_empty())
        .map(|w| w.to_string())
        .collect()
}

pub fn split_propositions(input: &str) -> Vec<String> {
    let mut results = Vec::new();
    let mut current = String::new();
    let chars: Vec<char> = input.chars().collect();
    let len = chars.len();
    let mut i = 0;
    let mut in_bracket = false;

    while i < len {
        let c = chars[i];
        if c == '\\' && i + 1 < len {
            current.push(c);
            current.push(chars[i + 1]);
            i += 2;
            continue;
        }

        if c == '[' {
            in_bracket = true;
            current.push(c);
            i += 1;
            continue;
        } else if c == ']' {
            in_bracket = false;
            current.push(c);
            i += 1;
            continue;
        }

        if !in_bracket {
            if c == ';' {
                results.push(std::mem::take(&mut current));
                i += 1;
                continue;
            } else if c == '|' {
                if i + 1 < len && chars[i + 1] == '|' {
                    i += 1;
                }
                results.push(std::mem::take(&mut current));
                i += 1;
                continue;
            } else if (c == ' ' || c == '\t') && i + 3 < len {
                let slice: String = chars[i..=(i + 3)].iter().collect::<String>().to_lowercase();
                if slice == " ou " || slice == " or " {
                    results.push(std::mem::take(&mut current));
                    i += 4;
                    continue;
                }
            }
        }

        current.push(c);
        i += 1;
    }

    if !current.trim().is_empty() {
        results.push(current);
    }

    results
}

pub fn extract_target_propositions(query: &str) -> Vec<String> {
    let mut propositions = Vec::new();
    for line in query.lines() {
        let line_trimmed = line.trim();
        if line_trimmed.is_empty() {
            continue;
        }

        if line_trimmed.starts_with('/') && (line_trimmed.ends_with('/') || line_trimmed.ends_with("/i")) {
            propositions.push(line_trimmed.to_string());
            continue;
        }

        let parts = split_propositions(line_trimmed);
        for part in parts {
            let p = part.trim();
            if !p.is_empty() {
                propositions.push(p.to_string());
            }
        }
    }

    if propositions.is_empty() && !query.trim().is_empty() {
        propositions.push(query.trim().to_string());
    }

    propositions
}

fn expand_grouped_alternations(pattern: &str) -> Vec<String> {
    if let Some(open) = pattern.find('(') {
        if let Some(close) = pattern[open..].find(')') {
            let close = open + close;
            let inside = &pattern[open + 1..close];
            if inside.contains('|') {
                let prefix = &pattern[..open];
                let suffix = &pattern[close + 1..];
                let mut res = Vec::new();
                for opt in inside.split('|') {
                    let sub = format!("{prefix}{opt}{suffix}");
                    res.extend(expand_grouped_alternations(&sub));
                }
                return res;
            }
        }
    }
    vec![pattern.to_string()]
}

fn split_pattern_branches(pattern: &str) -> Vec<String> {
    let mut branches = Vec::new();
    let mut cur = String::new();
    let mut in_bracket = false;
    let chars: Vec<char> = pattern.chars().collect();
    let len = chars.len();
    let mut i = 0;

    while i < len {
        let c = chars[i];
        if c == '\\' && i + 1 < len {
            cur.push(c);
            cur.push(chars[i + 1]);
            i += 2;
            continue;
        }
        if c == '[' {
            in_bracket = true;
            cur.push(c);
        } else if c == ']' {
            in_bracket = false;
            cur.push(c);
        } else if c == '|' && !in_bracket {
            branches.push(std::mem::take(&mut cur));
        } else if c != '(' && c != ')' {
            cur.push(c);
        }
        i += 1;
    }
    if !cur.is_empty() {
        branches.push(cur);
    }
    if branches.is_empty() {
        branches.push(pattern.to_string());
    }
    branches
}

fn parse_pattern_branch(branch: &str) -> Vec<PatternAtom> {
    let chars: Vec<char> = branch.chars().collect();
    let len = chars.len();
    let mut atoms = Vec::new();
    let mut i = 0;

    while i < len {
        let c = chars[i];
        if c == '^' && i == 0 {
            atoms.push(PatternAtom { kind: AtomKind::AnchorStart, quant: Quantifier::Once });
            i += 1;
            continue;
        }
        if c == '$' && i == len - 1 {
            atoms.push(PatternAtom { kind: AtomKind::AnchorEnd, quant: Quantifier::Once });
            i += 1;
            continue;
        }

        let kind = if c == '\\' && i + 1 < len {
            i += 2;
            match chars[i - 1] {
                'd' => AtomKind::Char(CharClass::Digit),
                'D' => AtomKind::Char(CharClass::NotDigit),
                'w' => AtomKind::Char(CharClass::Word),
                'W' => AtomKind::Char(CharClass::NotWord),
                's' => AtomKind::Char(CharClass::Whitespace),
                'S' => AtomKind::Char(CharClass::NotWhitespace),
                esc => AtomKind::Char(CharClass::Literal(esc.to_ascii_lowercase())),
            }
        } else if c == '.' {
            i += 1;
            AtomKind::Char(CharClass::Any)
        } else if c == '[' {
            i += 1;
            let negated = if i < len && chars[i] == '^' {
                i += 1;
                true
            } else {
                false
            };
            let mut custom_chars = Vec::new();
            let mut custom_ranges = Vec::new();

            while i < len && chars[i] != ']' {
                if chars[i] == '\\' && i + 1 < len {
                    i += 1;
                    match chars[i] {
                        'd' => custom_ranges.push(('0', '9')),
                        'w' => {
                            custom_ranges.push(('a', 'z'));
                            custom_ranges.push(('0', '9'));
                            custom_chars.push('_');
                        }
                        's' => {
                            custom_chars.push(' ');
                            custom_chars.push('\t');
                            custom_chars.push('\r');
                            custom_chars.push('\n');
                        }
                        esc => custom_chars.push(esc.to_ascii_lowercase()),
                    }
                } else if i + 2 < len && chars[i + 1] == '-' && chars[i + 2] != ']' {
                    let start = chars[i].to_ascii_lowercase();
                    let end = chars[i + 2].to_ascii_lowercase();
                    custom_ranges.push((start, end));
                    i += 2;
                } else {
                    custom_chars.push(chars[i].to_ascii_lowercase());
                }
                i += 1;
            }
            if i < len && chars[i] == ']' {
                i += 1;
            }
            AtomKind::Char(CharClass::Custom { chars: custom_chars, ranges: custom_ranges, negated })
        } else if c == '(' || c == ')' {
            i += 1;
            continue;
        } else {
            i += 1;
            AtomKind::Char(CharClass::Literal(c.to_ascii_lowercase()))
        };

        let quant = if i < len {
            match chars[i] {
                '*' => { i += 1; Quantifier::ZeroOrMore }
                '+' => { i += 1; Quantifier::OneOrMore }
                '?' => { i += 1; Quantifier::ZeroOrOne }
                _ => Quantifier::Once,
            }
        } else {
            Quantifier::Once
        };

        atoms.push(PatternAtom { kind, quant });
    }

    atoms
}

fn match_atoms(atoms: &[PatternAtom], atom_idx: usize, text: &[char], text_idx: usize) -> bool {
    match_atoms_bounded(atoms, atom_idx, text, text_idx, 0)
}

fn match_atoms_bounded(atoms: &[PatternAtom], atom_idx: usize, text: &[char], text_idx: usize, depth: usize) -> bool {
    if depth > 200 {
        return false;
    }
    if atom_idx >= atoms.len() {
        return true;
    }

    let atom = &atoms[atom_idx];
    match &atom.kind {
        AtomKind::AnchorStart => {
            if text_idx == 0 {
                match_atoms_bounded(atoms, atom_idx + 1, text, text_idx, depth + 1)
            } else {
                false
            }
        }
        AtomKind::AnchorEnd => {
            text_idx == text.len() && match_atoms_bounded(atoms, atom_idx + 1, text, text_idx, depth + 1)
        }
        AtomKind::Char(class) => match atom.quant {
            Quantifier::Once => {
                if text_idx < text.len() && class.matches(text[text_idx]) {
                    match_atoms_bounded(atoms, atom_idx + 1, text, text_idx + 1, depth + 1)
                } else {
                    false
                }
            }
            Quantifier::ZeroOrOne => {
                if text_idx < text.len() && class.matches(text[text_idx]) {
                    if match_atoms_bounded(atoms, atom_idx + 1, text, text_idx + 1, depth + 1) {
                        return true;
                    }
                }
                match_atoms_bounded(atoms, atom_idx + 1, text, text_idx, depth + 1)
            }
            Quantifier::ZeroOrMore => {
                let mut count = 0;
                while text_idx + count < text.len() && class.matches(text[text_idx + count]) {
                    count += 1;
                }
                for k in (0..=count).rev() {
                    if match_atoms_bounded(atoms, atom_idx + 1, text, text_idx + k, depth + 1) {
                        return true;
                    }
                }
                false
            }
            Quantifier::OneOrMore => {
                if text_idx >= text.len() || !class.matches(text[text_idx]) {
                    return false;
                }
                let mut count = 1;
                while text_idx + count < text.len() && class.matches(text[text_idx + count]) {
                    count += 1;
                }
                for k in (1..=count).rev() {
                    if match_atoms_bounded(atoms, atom_idx + 1, text, text_idx + k, depth + 1) {
                        return true;
                    }
                }
                false
            }
        },
    }
}

fn matches_single_regex(pattern: &str, text: &str) -> bool {
    let clean_pat = if pattern.starts_with('/') {
        let without_prefix = &pattern[1..];
        if let Some(stripped) = without_prefix.strip_suffix("/i") {
            stripped
        } else if let Some(stripped) = without_prefix.strip_suffix('/') {
            stripped
        } else {
            without_prefix
        }
    } else {
        pattern
    }.trim();

    if clean_pat.is_empty() {
        return false;
    }

    let expanded = expand_grouped_alternations(clean_pat);
    let text_chars: Vec<char> = text.chars().collect();

    for variant in expanded {
        let branches = split_pattern_branches(&variant);
        for branch in branches {
            let b_trim = branch.trim();
            if b_trim.is_empty() {
                continue;
            }
            let atoms = parse_pattern_branch(b_trim);
            if atoms.is_empty() {
                continue;
            }

            let is_anchored_start = matches!(atoms.first(), Some(PatternAtom { kind: AtomKind::AnchorStart, .. }));

            if is_anchored_start {
                if match_atoms(&atoms, 0, &text_chars, 0) {
                    return true;
                }
            } else {
                for start_pos in 0..=text_chars.len() {
                    if match_atoms(&atoms, 0, &text_chars, start_pos) {
                        return true;
                    }
                }
            }
        }
    }

    false
}

pub fn matches_pattern(pattern: &str, text: &str) -> bool {
    let p_trim = pattern.trim();
    if p_trim.is_empty() {
        return false;
    }

    if !p_trim.starts_with('/') && !p_trim.contains(['*', '+', '?', '^', '$', '\\', '[', '(', '|']) {
        return text.to_lowercase().contains(&p_trim.to_lowercase());
    }

    matches_single_regex(p_trim, text)
}