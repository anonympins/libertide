use std::collections::HashMap;
use std::sync::mpsc::{Receiver, Sender};
use std::time::Duration;
use crate::current_time_str;
use crate::types::{AgentEvent, TwitchChannelItem, TwitchMessage};

pub fn spawn_twitch_worker(event_tx: Sender<AgentEvent>, channel_rx: Receiver<String>) {
    std::thread::spawn(move || {
        use std::io::{BufRead, BufReader, ErrorKind, Write};
        use std::net::TcpStream;

        let parse_channels = |raw: &str| -> Vec<String> {
            raw.split([',', ';', ' '])
                .map(|s| s.trim().to_lowercase().trim_start_matches('#').to_string())
                .filter(|s| !s.is_empty())
                .collect()
        };

        let mut current_channels: Vec<String> = Vec::new();

        loop {
            // En attente d'un salon si aucun n'est configuré
            if current_channels.is_empty() {
                match channel_rx.recv() {
                    Ok(ch) => {
                        current_channels = parse_channels(&ch);
                        if current_channels.is_empty() {
                            continue;
                        }
                    }
                    Err(_) => return,
                }
            }

            // Récupérer le dernier salon demandé s'il y a eu plusieurs bascules
            while let Ok(ch) = channel_rx.try_recv() {
                let parsed = parse_channels(&ch);
                if !parsed.is_empty() {
                    current_channels = parsed;
                }
            }

            if let Ok(stream) = TcpStream::connect("irc.chat.twitch.tv:6667") {
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
                for ch in &current_channels {
                    let _ = write!(writer, "JOIN #{ch}\r\n");
                }
                let _ = writer.flush();

                let mut reader = BufReader::new(stream);
                let mut line = String::new();

                loop {
                    // Interrompre la connexion active si un nouveau salon est demandé
                    if let Ok(new_ch) = channel_rx.try_recv() {
                        let parsed = parse_channels(&new_ch);
                        if !parsed.is_empty() && parsed != current_channels {
                            current_channels = parsed;
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
                                    if let Some((target_channel, msg)) = msg_payload.split_once(" :") {
                                        let channel_name = target_channel.trim_start_matches('#');
                                        let display_author = if current_channels.len() > 1 {
                                            format!("{author} [#{channel_name}]")
                                        } else {
                                            author.to_string()
                                        };
                                        let _ = event_tx.send(AgentEvent::TwitchChatReceived(TwitchMessage {
                                            author: display_author,
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

/// Vérifie l'état en direct ("open" / LIVE) et retourne un tuple (login, is_live)
pub async fn check_channels_live_status(channels: &[String]) -> Vec<(String, bool)> {
    let details = check_channels_live_details(channels).await;
    details.into_iter().map(|(login, (is_live, ..))| (login, is_live)).collect()
}

/// Vérifie en détail le statut live, le nombre de spectateurs et le titre pour une liste de logins
pub async fn check_channels_live_details(channels: &[String]) -> HashMap<String, (bool, Option<u32>, Option<String>)> {
    let mut map = HashMap::new();
    if channels.is_empty() {
        return map;
    }

    let client = reqwest::Client::new();
    let mut user_queries = String::new();
    for (i, ch) in channels.iter().enumerate() {
        let clean = ch.trim().to_lowercase().trim_start_matches('#').to_string();
        if !clean.is_empty() {
            user_queries.push_str(&format!(
                r#"u{}: user(login: "{}") {{ login stream {{ id viewersCount title }} broadcastSettings {{ title }} }} "#,
                i, clean
            ));
        }
    }

    if user_queries.is_empty() {
        return map;
    }

    let gql_body = serde_json::json!({
        "query": format!("query {{ {} }}", user_queries)
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
                if let Some(data) = v.get("data").and_then(|d| d.as_object()) {
                    for (_k, user_val) in data {
                        let login = user_val.get("login").and_then(|s| s.as_str()).unwrap_or("").to_lowercase();
                        let stream_val = user_val.get("stream");
                        let is_live = stream_val.map_or(false, |s| !s.is_null() && s.get("id").is_some());
                        let viewers = stream_val.and_then(|s| s.get("viewersCount")).and_then(|vc| vc.as_u64()).map(|n| n as u32);
                        let title = stream_val
                            .and_then(|s| s.get("title"))
                            .and_then(|t| t.as_str())
                            .or_else(|| user_val.get("broadcastSettings").and_then(|b| b.get("title")).and_then(|t| t.as_str()))
                            .map(|s| s.to_string())
                            .filter(|t| !t.trim().is_empty());

                        if !login.is_empty() {
                            map.insert(login, (is_live, viewers, title));
                        }
                    }
                }
            }
        }
    }
    map
}

pub async fn search_twitch_channels(query: &str) -> Vec<TwitchChannelItem> {
    let clean = query.trim().trim_start_matches('#');
    let client = reqwest::Client::new();

    if clean.is_empty() {
        // Requête GraphQL Twitch pour récupérer les 20 streams actuellement en direct
        let gql_body = serde_json::json!({
            "query": r#"query { streams(first: 20) { edges { node { id viewersCount title broadcaster { login displayName profileImageURL(width: 70) } } } } }"#
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
                    if let Some(edges) = v.pointer("/data/streams/edges").and_then(|e| e.as_array()) {
                        for edge in edges {
                            if let Some(node) = edge.get("node") {
                                let viewers = node.get("viewersCount").and_then(|vc| vc.as_u64()).map(|n| n as u32);
                                let title = node.get("title").and_then(|t| t.as_str()).map(|s| s.to_string());
                                if let Some(b) = node.get("broadcaster") {
                                    let login = b.get("login").and_then(|s| s.as_str()).unwrap_or("").to_string();
                                    let display_name = b.get("displayName").and_then(|s| s.as_str()).unwrap_or(&login).to_string();
                                    let profile_image_url = b.get("profileImageURL").and_then(|s| s.as_str()).map(|s| s.to_string());
                                    if !login.is_empty() {
                                        results.push(TwitchChannelItem {
                                            login,
                                            display_name,
                                            profile_image_url,
                                            is_live: true,
                                            viewers_count: viewers,
                                            stream_title: title,
                                        });
                                    }
                                }
                            }
                        }
                    }
                    if !results.is_empty() {
                        results.sort_by(|a, b| b.viewers_count.unwrap_or(0).cmp(&a.viewers_count.unwrap_or(0)));
                        return results;
                    }
                }
            }
        }

        // Repli : vérifier le statut en direct de streamers populaires
        let fallback_list = vec![
            "kamet0", "zerator", "squeezie", "aminematue", "gotaga", "antoinedaniel",
            "mistermv", "domingo", "ponce", "etoiles", "otplol_", "solary",
            "xqc", "ibai", "tarik", "shroud", "caseoh_", "gaules",
        ];
        let logins: Vec<String> = fallback_list.into_iter().map(String::from).collect();
        let live_details = check_channels_live_details(&logins).await;
        let mut results = Vec::new();
        for (login, (is_live, viewers, title)) in live_details {
            if is_live {
                results.push(TwitchChannelItem {
                    login: login.clone(),
                    display_name: login,
                    profile_image_url: None,
                    is_live: true,
                    viewers_count: viewers,
                    stream_title: title,
                });
            }
        }
        results.sort_by(|a, b| b.viewers_count.unwrap_or(0).cmp(&a.viewers_count.unwrap_or(0)));
        return results;
    }

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
                            results.push(TwitchChannelItem {
                                login,
                                display_name,
                                profile_image_url,
                                is_live: false,
                                viewers_count: None,
                                stream_title: None,
                            });
                        }
                    }
                }

                if !results.is_empty() {
                    // Résolution du statut LIVE et viewers pour tous les résultats trouvés
                    let logins: Vec<String> = results.iter().map(|r| r.login.clone()).collect();
                    let live_details = check_channels_live_details(&logins).await;
                    for item in &mut results {
                        if let Some((is_live, viewers, title)) = live_details.get(&item.login.to_lowercase()) {
                            item.is_live = *is_live;
                            item.viewers_count = *viewers;
                            item.stream_title = title.clone();
                        }
                    }
                    // Priorité aux chaînes en direct, puis tri par viewers décroissants
                    results.sort_by(|a, b| {
                        b.is_live
                            .cmp(&a.is_live)
                            .then_with(|| b.viewers_count.unwrap_or(0).cmp(&a.viewers_count.unwrap_or(0)))
                    });
                    return results;
                }
            }
        }
    }

    let live_details = check_channels_live_details(&[clean.to_string()]).await;
    let (is_live, viewers, title) = live_details.get(&clean.to_lowercase()).cloned().unwrap_or((false, None, None));

    // Repli automatique avec les informations saisies
    vec![TwitchChannelItem {
        login: clean.to_lowercase(),
        display_name: clean.to_string(),
        profile_image_url: None,
        is_live,
        viewers_count: viewers,
        stream_title: title,
    }]
}