use crate::Engine;
use serde_json::{Value, json};
use std::collections::HashMap;
use std::time::{Duration, Instant};

fn advertise(from: &Engine, target: &Engine) {
    from.command(&json!({"type":"discover_peer","device_id":target.device_id(),"display_name":"Test peer","os":"android","host":"127.0.0.1","port":target.port()}).to_string()).unwrap();
}

fn events(engine: &Engine) -> Vec<Value> {
    std::iter::from_fn(|| engine.poll())
        .map(|s| serde_json::from_str(&s).unwrap())
        .collect()
}

fn pair(from: &Engine, target: &Engine) {
    from.command(&json!({"type":"pair","device_id":target.device_id()}).to_string())
        .unwrap();
    let deadline = Instant::now() + Duration::from_secs(5);
    let mut pin = None;
    let mut entry = None;
    let mut ready = [false; 2];
    while Instant::now() < deadline {
        for (index, engine) in [from, target].into_iter().enumerate() {
            for event in events(engine) {
                if event["type"] == "pairing" {
                    match event["kind"].as_str().unwrap_or_default() {
                        "consent" => engine.command(&json!({"type":"consent","attempt_id":event["attempt_id"],"accept":true}).to_string()).unwrap(),
                        "show_pin" => pin = Some(event["pin"].as_str().unwrap().to_owned()),
                        "enter_pin" => entry = event["attempt_id"].as_u64(),
                        "complete" => ready[index] = true,
                        _ => {},
                    }
                }
            }
        }
        if let (Some(pin), Some(attempt)) = (pin.as_ref(), entry.take()) {
            from.command(&json!({"type":"pin","attempt_id":attempt,"pin":pin}).to_string())
                .unwrap();
        }
        if ready == [true, true] {
            return;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    panic!("pairing failed, ready={ready:?}");
}

#[test]
fn four_authenticated_devices_route_audio_and_survive_first_device_shutdown() {
    let directory =
        std::env::temp_dir().join(format!("mineshare-android-test-{}", uuid::Uuid::new_v4()));
    let mut engines: Vec<_> = (0..4)
        .map(|i| {
            Some(
                Engine::start(
                    &directory.join(i.to_string()),
                    &format!("Device {i}"),
                    1440,
                    3120,
                )
                .unwrap(),
            )
        })
        .collect();
    for i in 0..4 {
        for j in 0..4 {
            if i != j {
                advertise(engines[i].as_ref().unwrap(), engines[j].as_ref().unwrap());
            }
        }
    }
    for i in 0..4 {
        for j in i + 1..4 {
            pair(engines[i].as_ref().unwrap(), engines[j].as_ref().unwrap());
        }
    }
    let sender = engines[2].as_ref().unwrap();
    let receiver = engines[1].as_ref().unwrap();
    sender.command(&json!({"type":"audio_route","device_id":receiver.device_id(),"send":true,"receive":true}).to_string()).unwrap();
    // Stopping the first machine leaves the independently paired route alive.
    drop(engines[0].take());
    let sender = engines[2].as_ref().unwrap();
    let receiver = engines[1].as_ref().unwrap();
    let deadline = Instant::now() + Duration::from_secs(3);
    let mut heard = false;
    while Instant::now() < deadline {
        let samples: Vec<_> = (0..1920)
            .map(|n| ((n / 2) as f32 * 0.0576).sin() * 0.2)
            .collect();
        sender.send_pcm(samples);
        std::thread::sleep(Duration::from_millis(20));
        if let Some(frame) = receiver.poll_audio()
            && frame.iter().any(|v| v.abs() > 0.03)
        {
            heard = true;
            break;
        }
    }
    assert!(
        heard,
        "authenticated Opus audio should continue between remaining devices"
    );
    receiver.command(&json!({"type":"audio_route","device_id":sender.device_id(),"send":false,"receive":false}).to_string()).unwrap();
    assert!(
        receiver.poll_audio().is_none(),
        "disabling reception clears queued audio"
    );
    let first_id = {
        let original: Value =
            serde_json::from_slice(&std::fs::read(directory.join("0/identity.json")).unwrap())
                .unwrap();
        original["device_id"].clone()
    };
    let restarted = Engine::start(&directory.join("0"), "Device 0", 1440, 3120).unwrap();
    assert_eq!(json!(restarted.device_id()), first_id);
    for engine in engines.iter().flatten() {
        advertise(engine, &restarted);
        advertise(&restarted, engine);
    }
    let deadline = Instant::now() + Duration::from_secs(7);
    let mut connected = HashMap::new();
    while Instant::now() < deadline {
        for event in events(&restarted) {
            if event["type"] == "snapshot" {
                for peer in event["peers"].as_array().unwrap() {
                    connected.insert(
                        peer["device_id"].as_str().unwrap().to_owned(),
                        peer["connected"].as_bool().unwrap(),
                    );
                }
            }
        }
        if connected.values().filter(|v| **v).count() == 3 {
            break;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    assert_eq!(
        connected.values().filter(|v| **v).count(),
        3,
        "all saved pairs reconnect without another PIN"
    );
    drop(restarted);
    drop(engines);
    std::fs::remove_dir_all(directory).unwrap();
}

#[test]
fn malformed_commands_and_stale_pairing_answers_are_rejected() {
    let directory =
        std::env::temp_dir().join(format!("mineshare-android-test-{}", uuid::Uuid::new_v4()));
    let engine = Engine::start(&directory, "Phone", 1440, 3120).unwrap();
    assert!(
        engine
            .command(r#"{"type":"pin","attempt_id":1,"pin":"123456"}"#)
            .is_err()
    );
    assert!(
        engine
            .command(r#"{"type":"display","width":0,"height":3120}"#)
            .is_err()
    );
    assert!(
        engine
            .command(r#"{"type":"pair","device_id":"unknown"}"#)
            .is_err()
    );
    engine.command(r#"{"type":"pause","paused":true}"#).unwrap();
    assert!(
        engine
            .command(r#"{"type":"pair","device_id":"unknown"}"#)
            .is_err()
    );
    drop(engine);
    std::fs::remove_dir_all(directory).unwrap();
}

#[test]
fn optional_lock_visual_has_a_stable_positional_wire_encoding() {
    let ping = mineshare_protocol::ControlMsg::Ping {
        ts_us: 42,
        lock_visual: None,
    };
    let encoded = bincode::serde::encode_to_vec(&ping, bincode::config::standard()).unwrap();
    let (decoded, used) = bincode::serde::decode_from_slice::<mineshare_protocol::ControlMsg, _>(
        &encoded,
        bincode::config::standard(),
    )
    .unwrap();
    assert_eq!(used, encoded.len());
    assert!(matches!(
        decoded,
        mineshare_protocol::ControlMsg::Ping {
            ts_us: 42,
            lock_visual: None
        }
    ));
}

#[test]
fn pausing_during_pin_display_closes_pairing_without_trusting_the_requester() {
    let directory =
        std::env::temp_dir().join(format!("mineshare-android-test-{}", uuid::Uuid::new_v4()));
    let first = Engine::start(&directory.join("first"), "Laptop", 1920, 1080).unwrap();
    let phone = Engine::start(&directory.join("phone"), "Phone", 1440, 3120).unwrap();
    advertise(&first, &phone);
    first
        .command(&json!({"type":"pair","device_id":phone.device_id()}).to_string())
        .unwrap();
    let deadline = Instant::now() + Duration::from_secs(3);
    let mut showing = false;
    while Instant::now() < deadline {
        for event in events(&phone) {
            if event["kind"] == "consent" {
                phone
                    .command(
                        &json!({"type":"consent","attempt_id":event["attempt_id"],"accept":true})
                            .to_string(),
                    )
                    .unwrap();
            }
            if event["kind"] == "show_pin" {
                showing = true;
            }
        }
        if showing {
            break;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    assert!(showing);
    phone.command(r#"{"type":"pause","paused":true}"#).unwrap();
    let deadline = Instant::now() + Duration::from_secs(2);
    let mut closed = false;
    while Instant::now() < deadline {
        closed |= events(&first).iter().any(|v| v["type"] == "error");
        if closed {
            break;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    assert!(
        closed,
        "pause should close the pending encrypted setup immediately"
    );
    assert!(!directory.join("phone/peers.json").exists());
    assert!(
        phone
            .command(r#"{"type":"pin","attempt_id":1,"pin":"123456"}"#)
            .is_err()
    );
    drop(first);
    drop(phone);
    std::fs::remove_dir_all(directory).unwrap();
}

#[test]
fn ipv6_pair_uses_the_ipv6_audio_endpoint() {
    let directory =
        std::env::temp_dir().join(format!("mineshare-android-test-{}", uuid::Uuid::new_v4()));
    let first = Engine::start(&directory.join("first"), "Laptop", 1920, 1080).unwrap();
    let phone = Engine::start(&directory.join("phone"), "Phone", 1440, 3120).unwrap();
    first.command(&json!({"type":"discover_peer","device_id":phone.device_id(),"host":"::1","scope_id":0,"port":phone.port(),"compatible":true}).to_string()).unwrap();
    pair(&first, &phone);
    first.command(&json!({"type":"audio_route","device_id":phone.device_id(),"send":true,"receive":false}).to_string()).unwrap();
    let mut heard = false;
    for _ in 0..50 {
        first.send_pcm((0..1920).map(|n| (n as f32 * 0.06).sin() * 0.2).collect());
        std::thread::sleep(Duration::from_millis(20));
        if let Some(frame) = phone.poll_audio()
            && frame.iter().any(|s| s.abs() > 0.03)
        {
            heard = true;
            break;
        }
    }
    assert!(heard, "IPv6 authenticated audio did not arrive");
    drop(first);
    drop(phone);
    std::fs::remove_dir_all(directory).unwrap();
}

#[test]
fn trusted_reconnect_preserves_another_devices_pairing_prompt() {
    let directory =
        std::env::temp_dir().join(format!("mineshare-android-test-{}", uuid::Uuid::new_v4()));
    let first = Engine::start(&directory.join("first"), "Laptop", 1920, 1080).unwrap();
    let phone = Engine::start(&directory.join("phone"), "Phone", 1440, 3120).unwrap();
    let third = Engine::start(&directory.join("third"), "PC", 1920, 1080).unwrap();
    advertise(&first, &phone);
    pair(&first, &phone);
    let first_id = first.device_id();
    drop(first);
    std::thread::sleep(Duration::from_millis(50));
    events(&phone);
    advertise(&third, &phone);
    third
        .command(&json!({"type":"pair","device_id":phone.device_id()}).to_string())
        .unwrap();
    let mut attempt = None;
    let deadline = Instant::now() + Duration::from_secs(3);
    while Instant::now() < deadline {
        for event in events(&phone) {
            if event["kind"] == "consent" {
                attempt = event["attempt_id"].as_u64();
            }
        }
        if attempt.is_some() {
            break;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    let attempt = attempt.expect("third device did not request consent");
    let restarted = Engine::start(&directory.join("first"), "Laptop", 1920, 1080).unwrap();
    advertise(&restarted, &phone);
    let deadline = Instant::now() + Duration::from_secs(5);
    let mut reconnected = false;
    while Instant::now() < deadline {
        for event in events(&phone) {
            assert_ne!(
                event["kind"], "complete",
                "a trusted reconnect dismissed another device's prompt"
            );
            assert_ne!(
                event["kind"], "closed",
                "a trusted reconnect dismissed another device's prompt"
            );
            if let Some(peers) = event["peers"].as_array() {
                reconnected |= peers
                    .iter()
                    .any(|p| p["device_id"] == first_id && p["connected"] == true);
            }
        }
        if reconnected {
            break;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    assert!(reconnected);
    phone
        .command(&json!({"type":"consent","attempt_id":attempt,"accept":false}).to_string())
        .unwrap();
    drop(restarted);
    drop(third);
    drop(phone);
    std::fs::remove_dir_all(directory).unwrap();
}
