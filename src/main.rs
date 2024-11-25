#![doc = include_str!("../README.md")]

use libp2p::futures::StreamExt;
use libp2p::kad::store::MemoryStore;
use libp2p::kad::{self, Mode};
use libp2p::swarm::NetworkBehaviour;
use libp2p::{gossipsub, identity, PeerId};
use libp2p::{identify, mdns, noise, swarm::SwarmEvent, tcp, yamux};
use std::error::Error;
use std::hash::{DefaultHasher, Hash, Hasher};
use std::time::Duration;
use std::u64;
use tokio::{
    io::{self, AsyncBufReadExt},
    select,
};
use tracing_subscriber::EnvFilter;

const FRIEND_REQUEST_TOPIC: &str = "friend_requests";
const CHAT_ADD_TOPIC: &str = "add_chat";
const MSG_PING_TOPIC: &str = "msg_ping_topic";

#[tokio::main]
async fn main() -> Result<(), Box<dyn Error>> {
    std::env::set_var("RUST_LOG", "info");

    let _ = tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::from_default_env())
        .try_init();

    let keypair = identity::Keypair::generate_ed25519();
    let peer_id = PeerId::from(keypair.public());

    // Read full lines from stdin
    let mut stdin = io::BufReader::new(io::stdin()).lines();

    println!("Welcome to Conf-Chat!");
    println!("To get started, log-in with /login <username> <password>");
    println!("or register with /register <username> <password>");
    println!(
        "To change your password after logging in, type /update <old-password> <new-password>"
    );

    let mut username = peer_id.to_base58();
    let mut password = "password".to_string();
    if let Some(line) = stdin.next_line().await? {
        match line {
            line if line.starts_with("/login") => {
                if let Some(args) = line.strip_prefix("/login ") {
                    let tokens: Vec<&str> = args.split(' ').collect();
                    username = tokens[0].to_string();
                    password = tokens[1].to_string();
                }
            }
            line if line.starts_with("/register") => {
                if let Some(args) = line.strip_prefix("/register ") {
                    let tokens: Vec<&str> = args.split(' ').collect();
                    username = tokens[0].to_string();
                    password = tokens[1].to_string();
                }
            }
            _ => {
                // Defaults
            }
        }
    }

    let mut swarm = libp2p::SwarmBuilder::with_new_identity()
        .with_tokio()
        .with_tcp(
            tcp::Config::default(),
            noise::Config::new,
            yamux::Config::default,
        )?
        .with_behaviour(|key| {
            // Set a custom gossipsub configuration
            let gossipsub_config = gossipsub::ConfigBuilder::default()
                .heartbeat_interval(Duration::from_secs(10)) // This is set to aid debugging by not cluttering the log space
                .validation_mode(gossipsub::ValidationMode::Strict) // This sets the kind of message validation. The default is Strict (enforce message signing)
                .message_id_fn(|message: &gossipsub::Message| {
                    let mut s = DefaultHasher::new();
                    message.data.hash(&mut s);
                    message.sequence_number.hash(&mut s);
                    gossipsub::MessageId::from(s.finish().to_string())
                }) // content-address messages. No two messages of the same content will be propagated.
                .build()
                .map_err(|msg| io::Error::new(io::ErrorKind::Other, msg))
                .unwrap(); // Temporary hack because `build` does not return a proper `std::error::Error`.

            ConfChatBehavior {
                identify: identify::Behaviour::new(identify::Config::new(
                    "conf-chat/1.0.0".to_string(),
                    key.public(),
                )),
                kad: kad::Behaviour::new(
                    key.public().to_peer_id(),
                    MemoryStore::new(key.public().to_peer_id()),
                ),
                mdns: mdns::tokio::Behaviour::new(
                    mdns::Config::default(),
                    key.public().to_peer_id(),
                )
                .unwrap(),
                gossipsub: gossipsub::Behaviour::new(
                    gossipsub::MessageAuthenticity::Signed(key.clone()),
                    gossipsub_config,
                )
                .unwrap(),
            }
        })?
        .with_swarm_config(|cfg| cfg.with_idle_connection_timeout(Duration::from_secs(u64::MAX)))
        .build();

    swarm.behaviour_mut().kad.set_mode(Some(Mode::Server));

    let friend_request_topic = gossipsub::IdentTopic::new(FRIEND_REQUEST_TOPIC);
    let _ = swarm
        .behaviour_mut()
        .gossipsub
        .subscribe(&friend_request_topic);

    let chat_add_topic = gossipsub::IdentTopic::new(CHAT_ADD_TOPIC);
    let _ = swarm.behaviour_mut().gossipsub.subscribe(&chat_add_topic);

    let msg_ping_topic = gossipsub::IdentTopic::new(MSG_PING_TOPIC);
    let _ = swarm.behaviour_mut().gossipsub.subscribe(&msg_ping_topic);

    // Listen on all interfaces and whatever port the OS assigns.
    swarm.listen_on("/ip4/0.0.0.0/tcp/0".parse()?)?;

    // Here's where basic commands are provided
    println!("To get started, create a chatroom with /create <topic>");
    println!("Add users to chat rooms with /add <username>");
    println!("Users must be your friend to be added to a chatroom");
    println!("To view friends, use /viewFriends");
    println!("To add a friend, use /friend <username>");
    println!("To message a friend directly, use /msg <username> <message>");

    let mut friends: Vec<String> = vec![];
    let mut current_topic = "general".to_string();
    let _ = swarm
        .behaviour_mut()
        .gossipsub
        .subscribe(&gossipsub::IdentTopic::new(current_topic.clone()));

    loop {
        select! {
            Ok(Some(line)) = stdin.next_line() => {
                match line {
                    // Update password
                    line if line.starts_with("/update") => {
                        if let Some(remainder) = line.strip_prefix("/update ") {
                            let mut remainder = remainder.split(' ');
                            if remainder.next().unwrap() == password {
                                password = remainder.next().unwrap().to_string();
                                println!("Password updated!");
                            } else {
                                println!("Incorrect password!");
                            }
                        }
                    }
                    // Print out friends
                    line if line.starts_with("/viewFriends") => {
                        println!("{:?}", friends);
                    }
                    // Send a friend request
                    line if line.starts_with("/friend") => {
                        let _ = swarm.behaviour_mut().gossipsub.publish(friend_request_topic.clone(), format!("{line} {username}").as_bytes());
                    }
                    // Create a conference chat
                    line if line.starts_with("/create") => {
                        if let Some(topic) = line.strip_prefix("/create ") {
                            let _ = swarm
                                .behaviour_mut()
                                .gossipsub
                                .unsubscribe(&gossipsub::IdentTopic::new(current_topic.clone()));

                            current_topic = topic.to_owned();

                            let _ = swarm
                                .behaviour_mut()
                                .gossipsub
                                .subscribe(&gossipsub::IdentTopic::new(current_topic.clone()));
                        }
                    }
                    // Add a friend to a conference chat
                    line if line.starts_with("/add") => {
                        let _ = swarm.behaviour_mut().gossipsub.publish(chat_add_topic.clone(), format!("{line} {username} {}", current_topic).as_bytes());
                    }
                    // Message a friend
                    line if line.starts_with("/msg") => {
                        if let Some(remainder) = line.strip_prefix("/msg ") {
                            let mut remainder = remainder.split(' ');
                            let name = remainder.next().unwrap();
                            let message: String = remainder.collect::<Vec<&str>>().join(" ");
                            if friends.contains(&name.to_string()) {
                                let key = kad::RecordKey::new(&name);
                                let record = kad::Record { key, value: format!("{username} [Private Message]: {message}").as_bytes().to_vec(), publisher: Some(peer_id), expires: None };
                                let _ = swarm.behaviour_mut().kad.put_record(record, kad::Quorum::One);

                                let _ = swarm.behaviour_mut().gossipsub.publish(msg_ping_topic.clone(), format!("/msg {name} {username} {message}").as_bytes());
                            }
                        }
                    }
                    _ => {
                        // Send message in current chat
                        let _ = swarm.behaviour_mut().gossipsub.publish(gossipsub::IdentTopic::new(current_topic.clone()), format!("{username} [{current_topic}]: {line}").as_bytes());
                    }
                }
            }
            event = swarm.select_next_some() => match event {
                SwarmEvent::NewListenAddr { listener_id, address } => {
                    tracing::info!("{} is listening on {}", listener_id, address);
                },
                SwarmEvent::ConnectionEstablished { peer_id, .. } => {
                    tracing::info!("Connected to {}", peer_id);
                },
                SwarmEvent::ConnectionClosed {peer_id, cause, .. } => {
                    tracing::info!("Disconnected from {} due to error: {:?}", peer_id, cause);
                },
                SwarmEvent::Behaviour(ConfChatBehaviorEvent::Mdns(mdns::Event::Discovered(list))) => {
                    for (peer_id, multiaddr) in list {
                        swarm.behaviour_mut().kad.add_address(&peer_id, multiaddr);
                        tracing::info!("Added {} to known addresses", peer_id);

                        // Attempt to read offline messages
                        swarm
                            .behaviour_mut()
                            .kad
                            .get_record(kad::RecordKey::new(&username));
                    }
                },
                SwarmEvent::Behaviour(ConfChatBehaviorEvent::Mdns(mdns::Event::Expired(list))) => {
                    for (peer_id, multiaddr) in list {
                        swarm.behaviour_mut().kad.remove_address(&peer_id, &multiaddr);
                        tracing::info!("mDNS discover peer has expired: {peer_id}");
                    }
                },
                SwarmEvent::Behaviour(ConfChatBehaviorEvent::Gossipsub(gossipsub::Event::Message {
                    propagation_source: peer_id,
                    message_id: id,
                    message,
                })) => {
                    let data = String::from_utf8_lossy(&message.data);

                    tracing::info!("Got message: '{data}' with id: {id} from peer: {peer_id}");

                    // Check if it's a friend request
                    // format is {/friend <username> <requesterUsername>}
                    if data.starts_with("/friend") {
                        if let Some(remainder) = data.strip_prefix("/friend ") {
                            let mut remainder = remainder.split(' ');
                            if remainder.next().unwrap() == username.as_str() {
                                friends.push(remainder.next().unwrap().to_string());
                            }
                        }
                    }

                    // Check if it's a msg ping
                    // format is {/msg <username> <messagerUsername>}
                    else if data.starts_with("/msg") {
                        if let Some(remainder) = data.strip_prefix("/msg ") {
                            let mut remainder = remainder.split(' ');
                            let myname = remainder.next().unwrap();
                            let messager = remainder.next().unwrap();
                            if myname == username.as_str() && friends.contains(&messager.to_string()) {
                                let key = kad::RecordKey::new(&username);
                                let _ = swarm.behaviour_mut().kad.get_record(key);
                            }
                        }
                    }

                    // Check if it's a chat add request
                    // format is {/add <usernameToAdd> <requesterUsername> <chatTopic>}
                    else if data.starts_with("/add") {
                        if let Some(remainder) = data.strip_prefix("/add ") {
                            let mut remainder = remainder.split(' ');
                            if remainder.next().unwrap() == username.as_str() && friends.contains(&remainder.next().unwrap().to_string()) {
                                let _ = swarm
                                    .behaviour_mut()
                                    .gossipsub
                                    .unsubscribe(&gossipsub::IdentTopic::new(current_topic.clone()));


                                current_topic = remainder.next().unwrap().to_owned();

                                let _ = swarm
                                    .behaviour_mut()
                                    .gossipsub
                                    .subscribe(&gossipsub::IdentTopic::new(current_topic.clone()));
                            }
                        }
                    }

                    else {
                        println!("{data}");
                    }
                },
                SwarmEvent::Behaviour(ConfChatBehaviorEvent::Kad(kad::Event::OutboundQueryProgressed { result, ..})) => {
                    match result {
                        kad::QueryResult::GetProviders(Ok(kad::GetProvidersOk::FoundProviders { key, providers, .. })) => {
                            for peer in providers {
                                tracing::info!(
                                    "Peer {peer:?} provides key {:?}",
                                    std::str::from_utf8(key.as_ref()).unwrap()
                                );
                            }
                        }
                        kad::QueryResult::GetProviders(Err(err)) => {
                            tracing::error!("Failed to get providers: {err:?}");
                        }
                        kad::QueryResult::GetRecord(Ok(
                            kad::GetRecordOk::FoundRecord(kad::PeerRecord {
                                record: kad::Record { key, value, .. },
                                ..
                            })
                        )) => {
                            let message = std::str::from_utf8(&value).unwrap();

                            tracing::info!(
                                "Got record {:?} {:?}",
                                std::str::from_utf8(key.as_ref()).unwrap(),
                                message,
                            );
                            println!("{message}");
                        }
                        kad::QueryResult::GetRecord(Ok(_)) => {}
                        kad::QueryResult::GetRecord(Err(err)) => {
                            // This is an info and not error level log because we do lookups for possibly nonexistent records on purpose
                            tracing::info!("Failed to get record: {err:?}");
                        }
                        kad::QueryResult::PutRecord(Ok(kad::PutRecordOk { key })) => {
                            tracing::info!(
                                "Successfully put record {:?}",
                                std::str::from_utf8(key.as_ref()).unwrap()
                            );
                        }
                        kad::QueryResult::PutRecord(Err(err)) => {
                            tracing::error!("Failed to put record: {err:?}");
                        }
                        kad::QueryResult::StartProviding(Ok(kad::AddProviderOk { key })) => {
                            tracing::info!(
                                "Successfully put provider record {:?}",
                                std::str::from_utf8(key.as_ref()).unwrap()
                            );
                        }
                        kad::QueryResult::StartProviding(Err(err)) => {
                            tracing::error!("Failed to put provider record: {err:?}");
                        }
                        _ => {}
                    }
                },
                other => {
                    tracing::debug!("Unhandled {:?}", other);
                }
            }
        }
    }
}

#[derive(NetworkBehaviour)]
struct ConfChatBehavior {
    identify: identify::Behaviour,
    kad: kad::Behaviour<MemoryStore>,
    mdns: mdns::tokio::Behaviour,
    gossipsub: gossipsub::Behaviour,
}

// fn handle_command(
//     line: &String,
//     kademlia: &mut kad::Behaviour<MemoryStore>,
// ) -> Result<QueryId, String> {
//     let tokens: Vec<&str> = line.split_whitespace().collect();

//     match tokens[0] {
//         // "/login" => {
//         //     if tokens.len() != 3 {
//         //         return Err(format!(
//         //             "Invalid login, expected {} arguments but found {}",
//         //             2,
//         //             tokens.len() - 1
//         //         ));
//         //     }

//         //     return Ok(kademlia.get_record(kad::RecordKey::new(&tokens[1])));
//         // },
//         "/"

//         _ => {
//             return Err(format!("Invalid command: {}", line));
//         }
//     }

//     Ok(())
// }
