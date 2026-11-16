use crate::floating_window::FloatingCard;
use crate::overlay::normalize_for_command_match;

/// Extrait l'intention réelle et les mots-clés d'une recherche en retirant les tournures
/// conversationnelles, les injonctions orales ("cherche-moi", "peux-tu trouver") et les politesses.
pub fn clean_search_query_intent(query: &str) -> String {
    let mut s = query.trim();
    if s.is_empty() {
        return String::new();
    }

    while (s.starts_with('"') && s.ends_with('"') && s.len() >= 2)
        || (s.starts_with('\'') && s.ends_with('\'') && s.len() >= 2)
        || (s.starts_with('«') && s.ends_with('»') && s.len() >= 2)
    {
        s = s[1..s.len() - 1].trim();
    }

    let mut working = s.to_string();

    // 1. Nettoyage des suffixes de politesse et de plateforme à la fin de la phrase
    const SUFFIXES: &[&str] = &[
        "s'il te plaît", "s'il te plait", "s il te plait", "s il te plaît",
        "s'il vous plaît", "s'il vous plait", "s il vous plait", "s il vous plaît",
        "stp", "svp", "merci beaucoup", "merci d'avance", "merci",
        "please", "thank you", "thanks",
        "sur google", "sur internet", "sur youtube", "sur le web", "sur le net",
        "en ligne", "online",
    ];

    let mut changed = true;
    while changed {
        changed = false;
        let trimmed_end = working.trim_end_matches(|c: char| c == '?' || c == '!' || c == '.' || c == ',' || c == ';' || c == ':' || c.is_whitespace());
        if trimmed_end.len() < working.len() {
            working = trimmed_end.to_string();
            changed = true;
        }

        let lower = working.to_lowercase();
        for &sfx in SUFFIXES {
            if lower.ends_with(sfx) {
                let cut_idx = working.len() - sfx.len();
                working = working[..cut_idx].trim().to_string();
                changed = true;
                break;
            }
        }
    }

    // 2. Nettoyage des préfixes conversationnels au début de la phrase
    const PREFIXES: &[&str] = &[
        "peux-tu me faire une recherche sur",
        "peux-tu faire une recherche sur",
        "pourrais-tu me chercher",
        "pourrais-tu chercher",
        "pourrais-tu me trouver",
        "pourrais-tu trouver",
        "peux-tu me chercher",
        "peux-tu me trouver",
        "peux-tu chercher",
        "peux-tu trouver",
        "peux-tu me dire",
        "peux-tu regarder",
        "fais une recherche sur internet pour",
        "fais une recherche sur internet de",
        "fais une recherche sur google pour",
        "fais une recherche sur google de",
        "fais une recherche sur google",
        "fais une recherche sur internet",
        "fais une recherche sur le net",
        "fais une recherche sur le web",
        "fais une recherche sur",
        "fais une recherche de",
        "fais une recherche pour",
        "fais une recherche",
        "lance une recherche sur google pour",
        "lance une recherche sur google de",
        "lance une recherche sur google",
        "lance une recherche sur internet",
        "lance une recherche sur",
        "lance une recherche de",
        "lance une recherche",
        "va sur google et cherche",
        "va sur internet et cherche",
        "va voir sur google",
        "va voir sur internet",
        "va voir",
        "cherche-moi sur internet",
        "cherche moi sur internet",
        "cherche-moi sur google",
        "cherche moi sur google",
        "cherche-moi sur le net",
        "cherche moi sur le net",
        "cherche-moi sur le web",
        "cherche moi sur le web",
        "cherche sur internet",
        "cherche sur google",
        "cherche sur le net",
        "cherche sur le web",
        "cherche-moi",
        "cherche moi",
        "cherche",
        "chercher",
        "recherche-moi sur internet",
        "recherche moi sur internet",
        "recherche-moi sur google",
        "recherche moi sur google",
        "recherche sur internet",
        "recherche sur google",
        "recherche sur le net",
        "recherche-moi",
        "recherche moi",
        "recherche",
        "rechercher",
        "trouve-moi sur internet",
        "trouve moi sur internet",
        "trouve-moi sur google",
        "trouve moi sur google",
        "trouve-moi",
        "trouve moi",
        "trouve",
        "trouver",
        "regarde sur internet pour",
        "regarde sur google pour",
        "regarde sur internet",
        "regarde sur google",
        "regarde sur le net",
        "regarde",
        "je voudrais savoir",
        "je voudrais trouver",
        "je voudrais chercher",
        "je souhaite savoir",
        "j'aimerais savoir",
        "j'aimerais trouver",
        "j aimerais savoir",
        "j aimerais trouver",
        "je veux savoir",
        "je veux trouver",
        "je cherche",
        "dis-moi",
        "dis moi",
        "donne-moi",
        "donne moi",
        "affiche-moi",
        "affiche moi",
        "montre-moi",
        "montre moi",
        "mets-moi une vidéo de",
        "mets moi une vidéo de",
        "mets-moi une video de",
        "mets moi une video de",
        "mets une vidéo de",
        "mets une video de",
        "mets la vidéo de",
        "mets la video de",
        "mets-moi un film sur",
        "mets moi un film sur",
        "mets un film sur",
        "lance-moi une vidéo de",
        "lance moi une vidéo de",
        "lance une vidéo de",
        "lance une video de",
        "mets de la musique de",
        "mets la musique de",
        "mets-moi de la musique de",
        "mets moi de la musique de",
        "mets-moi",
        "mets moi",
        "mets",
        "lance-moi",
        "lance moi",
        "lance",
        "lancer",
        "ouvre",
        "ouvrir",
        "can you please search for",
        "can you search google for",
        "can you search for",
        "could you search for",
        "can you find me",
        "could you find me",
        "can you find",
        "could you find",
        "please search google for",
        "please search for",
        "search google for",
        "search for",
        "search",
        "look up on google",
        "look up",
        "find me",
        "find",
        "google for",
        "google",
        "tell me about",
        "what is the",
        "what is",
        "how to",
    ];

    let mut prefix_changed = true;
    while prefix_changed {
        prefix_changed = false;
        let lower = working.to_lowercase();
        for &pfx in PREFIXES {
            if lower.starts_with(pfx) {
                let rest = working[pfx.len()..].trim_start_matches(|c: char| c == ':' || c == '-' || c.is_whitespace());
                if !rest.is_empty() {
                    working = rest.to_string();
                    prefix_changed = true;
                    break;
                }
            }
        }
    }

    // 3. Normalisation des questions météo courantes vers des mots-clés d'intention directs
    let lower = working.to_lowercase();
    const WEATHER_PATTERNS: &[(&str, &str)] = &[
        ("quel temps fait-il à ", "météo "),
        ("quel temps fait-il a ", "météo "),
        ("quel temps fera-t-il à ", "météo "),
        ("quel temps fera-t-il a ", "météo "),
        ("quel temps il fait à ", "météo "),
        ("quel temps il fait a ", "météo "),
        ("quelle est la météo de ", "météo "),
        ("quelle est la météo à ", "météo "),
        ("quelle est la météo a ", "météo "),
        ("quelle est la météo pour ", "météo "),
        ("quelle est la meteo de ", "météo "),
        ("quelle est la meteo à ", "météo "),
        ("quelle est la meteo a ", "météo "),
        ("quelle est la meteo pour ", "météo "),
        ("la météo qu'il fera à ", "météo "),
        ("la météo qu'il fera a ", "météo "),
        ("la météo qu'il fera ", "météo "),
        ("la meteo qu'il fera à ", "météo "),
        ("la meteo qu'il fera a ", "météo "),
        ("la meteo qu'il fera ", "météo "),
        ("la météo de ", "météo "),
        ("la météo à ", "météo "),
        ("la météo a ", "météo "),
        ("la meteo de ", "météo "),
        ("la meteo à ", "météo "),
        ("la meteo a ", "météo "),
    ];

    for &(pat, repl) in WEATHER_PATTERNS {
        if lower.starts_with(pat) {
            working = format!("{repl}{}", &working[pat.len()..]);
            break;
        }
    }

    let result = working.trim().trim_matches(|c: char| c == '"' || c == '\'' || c == '«' || c == '»').trim();
    if result.is_empty() {
        s.to_string()
    } else {
        result.to_string()
    }
}

pub fn extract_badge_number(text: &str) -> Option<usize> {
    let lower = text.trim().to_lowercase();
    let clean = lower
        .trim_start_matches('#')
        .trim_start_matches("n°")
        .trim_start_matches("no.")
        .trim_start_matches("no ")
        .trim();

    if let Ok(n) = clean.parse::<usize>() {
        return Some(n);
    }

    match clean {
        "un" => return Some(1),
        "deux" => return Some(2),
        "trois" => return Some(3),
        "quatre" => return Some(4),
        "cinq" => return Some(5),
        "six" => return Some(6),
        "sept" => return Some(7),
        "huit" => return Some(8),
        "neuf" => return Some(9),
        "dix" => return Some(10),
        "onze" => return Some(11),
        "douze" => return Some(12),
        "treize" => return Some(13),
        "quatorze" => return Some(14),
        "quinze" => return Some(15),
        "seize" => return Some(16),
        "dix-sept" => return Some(17),
        "dix-huit" => return Some(18),
        "dix-neuf" => return Some(19),
        "vingt" => return Some(20),
        _ => {}
    }

    let words: Vec<&str> = lower.split_whitespace().collect();
    let is_badge_command = words.len() <= 6
        && (words.contains(&"badge")
            || words.contains(&"badges")
            || words.contains(&"numero")
            || words.contains(&"numéro")
            || words.contains(&"cible")
            || words.contains(&"element")
            || words.contains(&"élément")
            || words.contains(&"clic")
            || words.contains(&"clique")
            || words.contains(&"cliquer")
            || words.contains(&"click"));

    if is_badge_command || words.len() <= 2 {
        for word in words {
            let w_clean = word
                .trim_start_matches('#')
                .trim_start_matches("n°")
                .trim_start_matches("no.")
                .trim_end_matches('.')
                .trim_end_matches(',');
            if let Ok(n) = w_clean.parse::<usize>() {
                return Some(n);
            }
            match w_clean {
                "un" => return Some(1),
                "deux" => return Some(2),
                "trois" => return Some(3),
                "quatre" => return Some(4),
                "cinq" => return Some(5),
                "six" => return Some(6),
                "sept" => return Some(7),
                "huit" => return Some(8),
                "neuf" => return Some(9),
                "dix" => return Some(10),
                "onze" => return Some(11),
                "douze" => return Some(12),
                "treize" => return Some(13),
                "quatorze" => return Some(14),
                "quinze" => return Some(15),
                "seize" => return Some(16),
                "dix-sept" => return Some(17),
                "dix-huit" => return Some(18),
                "dix-neuf" => return Some(19),
                "vingt" => return Some(20),
                _ => {}
            }
        }
    }
    None
}

pub fn is_start_subtitles_command(norm: &str) -> bool {
    let localized_cmd = normalize_for_command_match(&t!("commands.start_subtitles").to_string());
    let localized_title = normalize_for_command_match(&t!("commands.subtitles_title").to_string());
    if !localized_cmd.is_empty() && (norm == localized_cmd || norm.contains(&localized_cmd)) {
        return true;
    }
    if !localized_title.is_empty() && (norm == localized_title || norm.contains(&localized_title)) {
        return true;
    }

    let kw_str = t!("commands.start_subtitles_keywords").to_string();
    for kw in kw_str.split(',') {
        let norm_kw = normalize_for_command_match(kw.trim());
        if !norm_kw.is_empty() && (norm == norm_kw || norm.contains(&norm_kw)) {
            return true;
        }
    }

    (norm.contains("active") || norm.contains("activer") || norm.contains("lance") || norm.contains("lancer") || norm.contains("met") || norm.contains("mettre") || norm.contains("affiche") || norm.contains("afficher") || norm.contains("ouvre") || norm.contains("ouvrir") || norm.contains("enable") || norm.contains("start") || norm.contains("turn on") || norm.contains("show"))
        && (norm.contains("sous titre") || norm.contains("sous-titre") || norm.contains("soustitre") || norm.contains("subtitles") || norm.contains("subtitle") || norm.contains("transcription en direct") || norm.contains("transcription live") || norm.contains("transcription audio"))
        || norm == "sous titres en direct" || norm == "sous-titres en direct" || norm == "sous titres" || norm == "sous-titres" || norm == "live subtitles"
        || norm.starts_with("sous titre") || norm.starts_with("sous-titre")
}

pub fn is_stop_subtitles_command(norm: &str) -> bool {
    let localized_cmd = normalize_for_command_match(&t!("commands.stop_subtitles").to_string());
    if !localized_cmd.is_empty() && (norm == localized_cmd || norm.contains(&localized_cmd)) {
        return true;
    }

    let kw_str = t!("commands.stop_subtitles_keywords").to_string();
    for kw in kw_str.split(',') {
        let norm_kw = normalize_for_command_match(kw.trim());
        if !norm_kw.is_empty() && (norm == norm_kw || norm.contains(&norm_kw)) {
            return true;
        }
    }

    (norm.contains("arrete") || norm.contains("arrête") || norm.contains("arreter") || norm.contains("arrêter")
        || norm.contains("stop") || norm.contains("stopper") || norm.contains("coupe") || norm.contains("couper")
        || norm.contains("ferme") || norm.contains("fermer") || norm.contains("desactive") || norm.contains("désactive")
        || norm.contains("desactiver") || norm.contains("désactiver") || norm.contains("quitte") || norm.contains("quitter")
        || norm.contains("disable") || norm.contains("turn off") || norm.contains("hide") || norm.contains("close"))
        && (norm.contains("sous titre") || norm.contains("sous-titre") || norm.contains("soustitre") || norm.contains("subtitles") || norm.contains("subtitle") || norm.contains("transcription"))
        || norm == "stop sous titres" || norm == "stop sous-titres" || norm == "stop subtitles"
}

pub fn is_read_card_intent(prompt: &str, floating_cards: &[FloatingCard]) -> Option<FloatingCard> {
    if floating_cards.is_empty() {
        return None;
    }
    let norm = prompt.trim();
    let lower = norm.to_lowercase();

    if lower == "lis la fenetre"
        || lower == "lis la fenêtre"
        || lower == "lis le resume"
        || lower == "lis le résumé"
        || lower == "lis la carte"
        || lower.starts_with("lis la fen")
        || lower.starts_with("lire la fen")
        || lower == "read window"
        || lower == "read card"
        || lower == "read the card"
        || lower == "read the window"
        || lower == "karte vorlesen"
        || lower == "fenster vorlesen"
        || lower == "leer ventana"
        || lower == "leer tarjeta"
    {
        return floating_cards.last().cloned();
    }

    if norm.starts_with('🔊') || norm.starts_with('🔉') || lower.starts_with("lire :") || lower.starts_with("read:") || lower.starts_with("leer:") || lower.starts_with("vorlesen:") || lower.starts_with("leggi:") || lower.starts_with("ler:") {
        let clean = norm.trim_start_matches(['🔊', '🔉']).trim();
        let target_title = if let Some((_, after)) = clean.split_once(':') {
            after.trim()
        } else if let Some((_, after)) = clean.split_once('：') {
            after.trim()
        } else {
            clean
        };
        if !target_title.is_empty() {
            if let Some(card) = floating_cards.iter().find(|c| c.title.eq_ignore_ascii_case(target_title) || c.id.eq_ignore_ascii_case(target_title)) {
                return Some(card.clone());
            }
        }
        return floating_cards.last().cloned();
    }
    None
}