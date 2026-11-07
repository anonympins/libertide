use crate::floating_window::FloatingCard;
use crate::overlay::normalize_for_command_match;

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