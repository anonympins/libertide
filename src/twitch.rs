use std::sync::mpsc::{Receiver, Sender};
use std::time::Duration;
use crate::current_time_str;
use crate::types::{AgentEvent, TwitchChannelItem, TwitchMessage};

pub fn spawn_twitch_worker(event_tx: Sender<AgentEvent>, channel_rx: Receiver<String>) {
    std::thread::spawn(move || {
        use std::io::{BufRead, BufReader, ErrorKind, Write};
        use std::net::TcpStream;

        let mut current_channel = String::new();

        loop {
            // En attente d'un salon si aucun n'est configuré
            if current_channel.is_empty() {
                match channel_rx.recv() {
                    Ok(ch) => {
                        current_channel = ch.trim().to_lowercase().trim_start_matches('#').to_string();
                        if current_channel.is_empty() {
                            continue;
                        }
                    }
                    Err(_) => return,
                }
            }

            // Récupérer le dernier salon demandé s'il y a eu plusieurs bascules
            while let Ok(ch) = channel_rx.try_recv() {
                let clean = ch.trim().to_lowercase().trim_start_matches('#').to_string();
                if !clean.is_empty() {
                    current_channel = clean;
                }
            }

            if let Ok(mut stream) = TcpStream::connect("irc.chat.twitch.tv:6667") {
                let _ = stream.set_read_timeout(Some(Duration::from_millis(500)));
                let mut writer = match stream.try_clone() {
                    Ok(w) => w,
                    Err(_) => {
                        std::thread::sleep(Duration::from_secs(3));
                        continue;
                    }
                };

                // Mode invité Twitch (anonyme sans OAuth en lecture seule)
                let _ = write!(writer, "PASS oauth:justinfan12345\r\n");
                let _ = write!(writer, "NICK justinfan12345\r\n");
                let _ = write!(writer, "JOIN #{current_channel}\r\n");
                let _ = writer.flush();

                let mut reader = BufReader::new(stream);
                let mut line = String::new();

                loop {
                    // Interrompre la connexion active si un nouveau salon est demandé
                    if let Ok(new_ch) = channel_rx.try_recv() {
                        let clean = new_ch.trim().to_lowercase().trim_start_matches('#').to_string();
                        if !clean.is_empty() && clean != current_channel {
                            current_channel = clean;
                            break;
                        }
                    }

                    line.clear();
                    match reader.read_line(&mut line) {
                        Ok(0) => break,
                        Ok(_) => {
                            let line_str = line.trim_end();
                            if line_str.starts_with("PING") {
                                let _ = write!(writer, "PONG :tmi.twitch.tv\r\n");
                                let _ = writer.flush();
                            } else if line_str.contains("PRIVMSG") {
                                if let Some(idx_privmsg) = line_str.find(" PRIVMSG ") {
                                    let prefix = &line_str[..idx_privmsg];
                                    let author = prefix.strip_prefix(':').unwrap_or(prefix).split('!').next().unwrap_or("anonyme");
                                    let msg_payload = &line_str[idx_privmsg + 9..];
                                    if let Some(colon_pos) = msg_payload.find(" :") {
                                        let msg = &msg_payload[colon_pos + 2..];
                                        let _ = event_tx.send(AgentEvent::TwitchChatReceived(TwitchMessage {
                                            author: author.to_string(),
                                            text: msg.to_string(),
                                            timestamp: current_time_str(),
                                        }));
                                    }
                                }
                            }
                        }
                        Err(ref e) if e.kind() == ErrorKind::WouldBlock || e.kind() == ErrorKind::TimedOut => {
                            continue;
                        }
                        Err(_) => break,
                    }
                }
            }
            std::thread::sleep(Duration::from_secs(5));
        }
    });
}

pub async fn search_twitch_channels(query: &str) -> Vec<TwitchChannelItem> {
    let clean = query.trim().trim_start_matches('#');
    if clean.is_empty() {
        return Vec::new();
    }

    let client = reqwest::Client::new();
    let gql_body = serde_json::json!({
        "query": format!(
            r#"query {{ searchFor(userQuery: "{}", platform: "web") {{ channels {{ items {{ id login displayName profileImageURL(width: 70) }} }} }} }}"#,
            clean.replace('"', "\\\"")
        )
    });

    let res = client
        .post("https://gql.twitch.tv/gql")
        .header("Client-Id", "kimne78kx3ncx6brgo4mv6wki5h1ko")
        .json(&gql_body)
        .send()
        .await;

    if let Ok(resp) = res {
        if resp.status().is_success() {
            if let Ok(v) = resp.json::<serde_json::Value>().await {
                let mut results = Vec::new();
                if let Some(items) = v.pointer("/data/searchFor/channels/items").and_then(|i| i.as_array()) {
                    for item in items {
                        let login = item.get("login").and_then(|s| s.as_str()).unwrap_or("").to_string();
                        let display_name = item.get("displayName").and_then(|s| s.as_str()).unwrap_or(&login).to_string();
                        let profile_image_url = item.get("profileImageURL").and_then(|s| s.as_str()).map(|s| s.to_string());
                        if !login.is_empty() {
                            results.push(TwitchChannelItem { login, display_name, profile_image_url });
                        }
                    }
                }
                if !results.is_empty() {
                    return results;
                }
            }
        }
    }

    // Repli automatique avec les informations saisies
    vec![TwitchChannelItem {
        login: clean.to_lowercase(),
        display_name: clean.to_string(),
        profile_image_url: None,
    }]
}