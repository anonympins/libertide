use std::io::{Read, Write};
use std::net::TcpStream;
use std::time::Duration;
use serde::{Deserialize, Serialize};
use rust_i18n::t;

use crate::fs_util::{read_env_var_from_file, write_env_var_to_file};

/// Entité Home Assistant résumée
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HaEntitySummary {
    pub entity_id: String,
    pub state: String,
    pub friendly_name: Option<String>,
}

/// Récupère l'URL configurée de Home Assistant (ex: http://homeassistant.local:8123)
pub fn get_home_assistant_url() -> String {
    std::env::var("HASS_URL")
        .ok()
        .or_else(|| read_env_var_from_file(".env", "HASS_URL"))
        .or_else(|| std::env::var("HOME_ASSISTANT_URL").ok())
        .or_else(|| read_env_var_from_file(".env", "HOME_ASSISTANT_URL"))
        .map(|s| s.trim().trim_end_matches('/').to_string())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "http://homeassistant.local:8123".to_string())
}

/// Récupère le jeton d'accès longue durée (Long-Lived Access Token) Home Assistant
pub fn get_home_assistant_token() -> String {
    std::env::var("HASS_TOKEN")
        .ok()
        .or_else(|| read_env_var_from_file(".env", "HASS_TOKEN"))
        .or_else(|| std::env::var("HOME_ASSISTANT_TOKEN").ok())
        .or_else(|| read_env_var_from_file(".env", "HOME_ASSISTANT_TOKEN"))
        .map(|s| s.trim().to_string())
        .unwrap_or_default()
}

/// Enregistre l'URL et le jeton Home Assistant dans le fichier `.env`
pub fn set_home_assistant_config(url: &str, token: &str) -> std::io::Result<()> {
    let clean_url = url.trim().trim_end_matches('/');
    let clean_token = token.trim();
    std::env::set_var("HASS_URL", clean_url);
    write_env_var_to_file(".env", "HASS_URL", clean_url)?;
    std::env::set_var("HASS_TOKEN", clean_token);
    write_env_var_to_file(".env", "HASS_TOKEN", clean_token)?;
    Ok(())
}

/// Extrait le couple (hôte, port) à partir d'une URL HTTP ou WebSocket
fn parse_host_port(url_str: &str, default_port: u16) -> (String, u16) {
    let without_scheme = url_str
        .strip_prefix("http://")
        .or_else(|| url_str.strip_prefix("ws://"))
        .or_else(|| url_str.strip_prefix("https://"))
        .or_else(|| url_str.strip_prefix("wss://"))
        .unwrap_or(url_str);

    let host_and_port = without_scheme.split('/').next().unwrap_or(without_scheme);
    if let Some((host, port_str)) = host_and_port.split_once(':') {
        let port = port_str.parse::<u16>().unwrap_or(default_port);
        (host.to_string(), port)
    } else {
        (host_and_port.to_string(), default_port)
    }
}

/// Envoie une trame WebSocket masquée côté client (RFC 6455)
fn send_ws_masked_text_frame(stream: &mut TcpStream, text: &str) -> std::io::Result<()> {
    let payload = text.as_bytes();
    let len = payload.len();
    let mut header = Vec::with_capacity(10);

    // Fin = 1, Opcode = 1 (Text)
    header.push(0x81);

    // Masque client obligatoire = 1 (0x80)
    if len <= 125 {
        header.push(0x80 | (len as u8));
    } else if len <= 65535 {
        header.push(0x80 | 126);
        header.extend_from_slice(&(len as u16).to_be_bytes());
    } else {
        header.push(0x80 | 127);
        header.extend_from_slice(&(len as u64).to_be_bytes());
    }

    // Clé de masquage 4 octets
    let mask_key: [u8; 4] = [0x4C, 0x49, 0x42, 0x54]; // 'LIBT'
    header.extend_from_slice(&mask_key);

    let mut masked_payload = Vec::with_capacity(len);
    for (i, &b) in payload.iter().enumerate() {
        masked_payload.push(b ^ mask_key[i % 4]);
    }

    stream.write_all(&header)?;
    stream.write_all(&masked_payload)?;
    stream.flush()?;
    Ok(())
}

/// Lit une trame texte WebSocket depuis le serveur Home Assistant
fn read_ws_frame(stream: &mut TcpStream) -> std::io::Result<String> {
    let mut hdr = [0u8; 2];
    stream.read_exact(&mut hdr)?;

    let payload_len_raw = hdr[1] & 0x7F;
    let payload_len: usize = if payload_len_raw <= 125 {
        payload_len_raw as usize
    } else if payload_len_raw == 126 {
        let mut ext = [0u8; 2];
        stream.read_exact(&mut ext)?;
        u16::from_be_bytes(ext) as usize
    } else {
        let mut ext = [0u8; 8];
        stream.read_exact(&mut ext)?;
        u64::from_be_bytes(ext) as usize
    };

    let is_masked = (hdr[1] & 0x80) != 0;
    let mask_key = if is_masked {
        let mut m = [0u8; 4];
        stream.read_exact(&mut m)?;
        Some(m)
    } else {
        None
    };

    let mut payload = vec![0u8; payload_len];
    stream.read_exact(&mut payload)?;

    if let Some(mask) = mask_key {
        for (i, b) in payload.iter_mut().enumerate() {
            *b ^= mask[i % 4];
        }
    }

    String::from_utf8(payload).map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))
}

/// Exécute un appel de service via le protocole WebSocket natif Home Assistant (`/api/websocket`)
pub fn call_home_assistant_websocket(
    domain: &str,
    service: &str,
    entity_id: Option<&str>,
    data: Option<&serde_json::Value>,
) -> Result<String, String> {
    let token = get_home_assistant_token();
    if token.is_empty() {
        return Err(t!("home_automation.token_missing").to_string());
    }

    let base_url = get_home_assistant_url();
    let (host, port) = parse_host_port(&base_url, 8123);

    let mut stream = TcpStream::connect((host.as_str(), port))
        .map_err(|e| format!("Connexion TCP impossible vers {host}:{port} : {e}"))?;
    stream.set_read_timeout(Some(Duration::from_millis(2500)))
        .map_err(|e| e.to_string())?;
    stream.set_write_timeout(Some(Duration::from_millis(2500)))
        .map_err(|e| e.to_string())?;

    // 1. Handshake HTTP Upgrade vers WebSocket
    let handshake_req = format!(
        "GET /api/websocket HTTP/1.1\r\n\
         Host: {host}:{port}\r\n\
         Upgrade: websocket\r\n\
         Connection: Upgrade\r\n\
         Sec-WebSocket-Key: dGhlIHNhbXBsZSBub25jZQ==\r\n\
         Sec-WebSocket-Version: 13\r\n\r\n"
    );
    stream.write_all(handshake_req.as_bytes()).map_err(|e| e.to_string())?;
    stream.flush().map_err(|e| e.to_string())?;

    // Lecture de la réponse de handshake HTTP 101 Switching Protocols
    let mut response_buf = [0u8; 1024];
    let bytes_read = stream.read(&mut response_buf).map_err(|e| e.to_string())?;
    let resp_str = String::from_utf8_lossy(&response_buf[..bytes_read]);
    if !resp_str.contains("101") && !resp_str.to_lowercase().contains("switching protocols") {
        return Err(format!("Échec du handshake WebSocket Home Assistant: {}", resp_str.lines().next().unwrap_or("")));
    }

    // 2. Réception du message auth_required
    let auth_req_msg = read_ws_frame(&mut stream).map_err(|e| format!("Lecture auth_required: {e}"))?;
    let v: serde_json::Value = serde_json::from_str(&auth_req_msg).unwrap_or_default();
    if v.get("type").and_then(|t| t.as_str()) != Some("auth_required") {
        return Err(format!("Message inattendu au démarrage WebSocket : {auth_req_msg}"));
    }

    // 3. Envoi du jeton d'authentification
    let auth_payload = serde_json::json!({
        "type": "auth",
        "access_token": token
    }).to_string();
    send_ws_masked_text_frame(&mut stream, &auth_payload).map_err(|e| format!("Envoi auth: {e}"))?;

    // 4. Validation auth_ok
    let auth_res_msg = read_ws_frame(&mut stream).map_err(|e| format!("Lecture auth_ok: {e}"))?;
    let auth_res: serde_json::Value = serde_json::from_str(&auth_res_msg).unwrap_or_default();
    if auth_res.get("type").and_then(|t| t.as_str()) != Some("auth_ok") {
        let err_reason = auth_res.get("message").and_then(|m| m.as_str()).unwrap_or(&auth_res_msg);
        return Err(format!("Authentification Home Assistant rejetée : {err_reason}"));
    }

    // 5. Envoi de l'ordre de service domotique
    let mut service_call = serde_json::json!({
        "id": 1,
        "type": "call_service",
        "domain": domain,
        "service": service,
    });

    if let Some(ent) = entity_id {
        service_call["target"] = serde_json::json!({ "entity_id": ent });
        service_call["service_data"] = serde_json::json!({ "entity_id": ent });
    }
    if let Some(extra_data) = data {
        if let Some(obj) = extra_data.as_object() {
            if let Some(sd) = service_call.get_mut("service_data").and_then(|d| d.as_object_mut()) {
                for (k, val) in obj {
                    sd.insert(k.clone(), val.clone());
                }
            } else {
                service_call["service_data"] = extra_data.clone();
            }
        }
    }

    send_ws_masked_text_frame(&mut stream, &service_call.to_string())
        .map_err(|e| format!("Envoi call_service: {e}"))?;

    // 6. Lecture du résultat
    let result_msg = read_ws_frame(&mut stream).map_err(|e| format!("Lecture résultat service: {e}"))?;
    let res_val: serde_json::Value = serde_json::from_str(&result_msg).unwrap_or_default();

    if res_val.get("success").and_then(|s| s.as_bool()).unwrap_or(false) {
        let all_str = t!("home_automation.all_entities").to_string();
        let ent_desc = entity_id.unwrap_or(&all_str);
        let domain_clean = domain;
        Ok(t!("home_automation.action_sent_ws", domain = domain_clean, service = service, entity = ent_desc).to_string())
    } else {
        let err_msg = res_val.pointer("/error/message").and_then(|m| m.as_str()).unwrap_or(&result_msg);
        Err(format!("Erreur Home Assistant : {err_msg}"))
    }
}

/// Exécute un appel de service via l'API REST locale (`POST /api/services/{domain}/{service}`)
pub fn call_home_assistant_rest(
    domain: &str,
    service: &str,
    entity_id: Option<&str>,
    data: Option<&serde_json::Value>,
) -> Result<String, String> {
    let token = get_home_assistant_token();
    if token.is_empty() {
        return Err(t!("home_automation.token_missing").to_string());
    }

    let base_url = get_home_assistant_url();
    let endpoint = format!("{base_url}/api/services/{domain}/{service}");

    let mut body = serde_json::Map::new();
    if let Some(ent) = entity_id {
        body.insert("entity_id".to_string(), serde_json::Value::String(ent.to_string()));
    }
    if let Some(extra_data) = data {
        if let Some(obj) = extra_data.as_object() {
            for (k, v) in obj {
                body.insert(k.clone(), v.clone());
            }
        }
    }

    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(|e| format!("Runtime Tokio: {e}"))?;

    let client = reqwest::Client::builder()
        .timeout(Duration::from_millis(3000))
        .build()
        .map_err(|e| format!("Client HTTP: {e}"))?;

    let res: Result<String, String> = rt.block_on(async {
        let resp = client
            .post(&endpoint)
            .bearer_auth(token.trim())
            .json(&serde_json::Value::Object(body))
            .send()
            .await
            .map_err(|e| format!("Connexion REST Home Assistant impossible : {e}"))?;

        if resp.status().is_success() {
            let all_str = t!("home_automation.all_entities").to_string();
            let ent_desc = entity_id.unwrap_or(&all_str);
            let domain_clean = domain;
            Ok(t!("home_automation.action_sent_rest", domain = domain_clean, service = service, entity = ent_desc).to_string())
        } else {
            let err_body = resp.text().await.unwrap_or_default();
            Err(format!("Erreur HTTP Home Assistant: {err_body}"))
        }
    });

    res
}

/// Normalise les services et verbes domotiques usuels dans toutes les langues supportées
pub fn normalize_ha_service(service: &str) -> &str {
    match service.trim().to_lowercase().as_str() {
        // Allumer / Activer
        "on" | "turn_on" | "turn-on" | "switch_on" | "enable" | "activate" | "start"
        | "allumer" | "activer" | "allume" | "active"
        | "einschalten" | "anschalten" | "an" | "ein" | "aktiviere" | "aktivieren"
        | "encender" | "activar" | "prender" | "enciende" | "activa"
        | "accendere" | "accendi" | "attivare" | "attiva"
        | "ligar" | "liga" | "acender" | "ativa"
        | "включить" | "включи" | "активировать"
        | "点灯" | "つける" | "点ける" | "オン" | "起動"
        | "打开" | "开启" | "开" => "turn_on",

        // Éteindre / Désactiver
        "off" | "turn_off" | "turn-off" | "switch_off" | "disable" | "deactivate"
        | "eteindre" | "éteindre" | "desactiver" | "désactiver" | "éteins" | "eteins" | "desactive" | "désactive"
        | "ausschalten" | "abschalten" | "aus" | "deaktivieren" | "deaktiviere"
        | "apagar" | "desactiva" | "apaga"
        | "spegnere" | "spegni" | "disattiva"
        | "desligar" | "desliga" | "desativa"
        | "выключить" | "выключи" | "отключить" | "погасить"
        | "消灯" | "けす" | "消す" | "オフ" | "停止"
        | "关闭" | "关掉" | "关" => "turn_off",

        // Ouvrir / Monter (volets, stores, rideaux)
        "open" | "open_cover" | "up" | "raise"
        | "ouvrir" | "ouvre" | "monter" | "monte" | "lever"
        | "öffnen" | "oeffnen" | "auf" | "hoch" | "hochfahren"
        | "abrir" | "abre" | "subir" | "sube"
        | "aprire" | "apri" | "alzare" | "alza" | "su"
        | "suba" | "levantar"
        | "открыть" | "открой" | "поднять" | "подними" | "вверх"
        | "開く" | "あける" | "開ける" | "上げる"
        | "升起" | "拉开" => "open_cover",

        // Fermer / Descendre (volets, stores, rideaux)
        "close" | "close_cover" | "down" | "lower"
        | "fermer" | "ferme" | "descendre" | "descends" | "baisser" | "baisse"
        | "schließen" | "schliessen" | "zu" | "runter" | "herunterfahren"
        | "cerrar" | "cierra" | "bajar" | "baja"
        | "chiudere" | "chiudi" | "abbassare" | "abbassa" | "giù" | "giu"
        | "fechar" | "fecha" | "descer" | "desça" | "abaixar"
        | "закрыть" | "закрой" | "опустить" | "опусти" | "вниз"
        | "閉じる" | "しめる" | "閉める" | "下げる"
        | "降下" | "拉上" | "合上" => "close_cover",

        // Arrêter / Stopper
        "stop" | "stop_cover" | "halt" | "pause"
        | "arreter" | "arrêter" | "arrête" | "arrete"
        | "stopp" | "anhalten"
        | "parar" | "para" | "detener" | "alto"
        | "fermare" | "ferma"
        | "deter"
        | "стоп" | "остановить" | "хватит"
        | "止める" | "ストップ"
        | "暂停" | "停" => "stop_cover",

        // Thermostat & Température
        "temperature" | "temp" | "température" | "set_temperature" | "thermostat" | "heat" | "cool"
        | "chauffer" | "climatiser" | "temperatur" | "grad" | "heizen" | "clima" | "grados" | "gradi" | "graus"
        | "температура" | "градус" | "градусы" | "温度" | "設定温度" | "设置温度" => "set_temperature",

        // Bascule (toggle)
        "toggle" | "basculer" | "bascule" | "umschalten" | "alternar" | "commuta" | "переключить" | "切替" | "切换" => "toggle",

        _ => service,
    }
}

/// Normalise les domaines d'équipements domotiques usuels dans toutes les langues supportées
pub fn normalize_ha_domain(domain: &str) -> &str {
    match domain.trim().to_lowercase().as_str() {
        // Éclairage & Lampes
        "light" | "lights" | "lamp" | "lamps" | "lighting"
        | "lumiere" | "lumière" | "lumières" | "lumieres" | "lampe" | "lampes" | "éclairage" | "eclairage"
        | "licht" | "lichter" | "beleuchtung"
        | "luz" | "luces" | "lámpara" | "lampara" | "iluminacion" | "iluminación"
        | "luce" | "luci" | "lampada" | "lampade" | "illuminazione"
        | "lâmpada" | "iluminação" | "iluminacao"
        | "свет" | "лампа" | "лампочка" | "освещение"
        | "照明" | "電気" | "ライト"
        | "灯" | "电灯" => "light",

        // Ouvrants, Volets & Stores
        "cover" | "covers" | "blind" | "blinds" | "shutter" | "shutters" | "curtain" | "curtains"
        | "volet" | "volets" | "store" | "stores" | "rideau" | "rideaux"
        | "rollladen" | "rolladen" | "jalousie" | "jalousien" | "rollo" | "rollos" | "vorhang"
        | "persiana" | "persianas" | "cortina" | "cortinas" | "toldo"
        | "tapparella" | "tapparelle" | "tenda" | "tende"
        | "estore" | "estores"
        | "жалюзи" | "шторы" | "рольставни" | "занавески"
        | "シャッター" | "ブラインド" | "カーテン"
        | "百叶窗" | "窗帘" | "卷帘" => "cover",

        // Thermostat & Climatisation
        "climate" | "hvac" | "thermostat" | "heater" | "heating" | "ac" | "air_conditioner"
        | "chauffage" | "clim" | "climatisation" | "radiateur"
        | "klima" | "klimaanlage" | "heizung"
        | "climatizador" | "calefaccion" | "calefacción" | "aire"
        | "climatizzatore" | "riscaldamento" | "aria_condizionata"
        | "ar_condicionado" | "aquecimento"
        | "климат" | "кондиционер"| "отопление"| "обогреватель"
        | "エアコン" | "暖房" | "冷房" | "空調" | "サーモスタット"
        | "空调" | "暖气" | "恒温器" | "温控器" => "climate",

        // Prises, Interrupteurs & Lit médicalisé
        "switch" | "switches" | "plug" | "plugs" | "socket" | "outlet" | "bed" | "medical_bed"
        | "prise" | "prises" | "interrupteur" | "interrupteurs" | "lit" | "lit_medicalise" | "lit_médicalisé"
        | "schalter" | "steckdose" | "steckdosen" | "bett" | "pflegebett"
        | "enchufe" | "enchufes" | "cama" | "cama_articulada"
        | "presa" | "prese" | "letto" | "letto_articolato"
        | "tomada" | "tomadas"
        | "розетка" | "выключатель" | "кровать" | "медицинская_кровать"
        | "コンセント" | "プラグ" | "ベッド"
        | "插座" | "床" | "护理床" => "switch",

        // Ventilateurs
        "fan" | "fans" | "ventilateur" | "ventilateurs" | "ventilator" | "lüfter" | "ventilador" | "ventilatore" | "вентилятор" | "扇風機" | "风扇" => "fan",

        // Boutons
        "button" | "bouton" | "knopf" | "botón" | "pulsante" | "botão" | "кнопка" | "ボタン" | "按钮" => "button",

        _ => domain,
    }
}

/// Point d'entrée universel pour exécuter une action de domotique avec bascule WebSocket -> REST
pub fn call_home_assistant_service(
    domain: &str,
    service: &str,
    entity_id: Option<&str>,
    data: Option<&serde_json::Value>,
) -> String {
    let norm_domain = normalize_ha_domain(domain);
    let norm_service = normalize_ha_service(service);

    // 1. Essai prioritaire via WebSocket direct
    match call_home_assistant_websocket(norm_domain, norm_service, entity_id, data) {
        Ok(msg) => {
            println!("[Home Automation] {msg}");
            msg
        }
        Err(ws_err) => {
            eprintln!("[Home Automation] Repli REST après échec WebSocket : {ws_err}");
            // 2. Repli immédiat sur l'API REST locale
            match call_home_assistant_rest(norm_domain, norm_service, entity_id, data) {
                Ok(msg) => {
                    println!("[Home Automation] {msg}");
                    msg
                }
                Err(rest_err) => {
                    let err = t!("home_automation.control_failed", error = rest_err).to_string();
                    eprintln!("[Home Automation] {err}");
                    err
                }
            }
        }
    }
}

/// Vérifie la joignabilité locale de Home Assistant
pub fn test_home_assistant_connection() -> Result<String, String> {
    let token = get_home_assistant_token();
    if token.is_empty() {
        return Err(t!("home_automation.token_missing").to_string());
    }
    let base_url = get_home_assistant_url();
    let endpoint = format!("{base_url}/api/");

    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(|e| e.to_string())?;

    let client = reqwest::Client::builder()
        .timeout(Duration::from_millis(2000))
        .build()
        .map_err(|e| e.to_string())?;

    rt.block_on(async {
        let resp = client.get(&endpoint).bearer_auth(token.trim()).send().await.map_err(|e| e.to_string())?;
        if resp.status().is_success() {
            Ok(t!("home_automation.test_ok", url = base_url).to_string())
        } else {
            Err(format!("Code statut HTTP {}", resp.status()))
        }
    })
}