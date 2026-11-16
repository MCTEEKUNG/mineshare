//! Compare the preserved broadcast/coalescer contract with production ingress.
use mineshare_daemon::input_pipeline::{InputHub, merge_motion};
use mineshare_input::{InputEvent, KeyCode, MAX_INJECT_DELTA_PX};
use std::{collections::VecDeque, time::Instant};

fn consume(mut events: VecDeque<InputEvent>, legacy: bool) -> (i64, Vec<(i64, bool)>, usize) {
    let mut distance = 0i64;
    let mut edges = Vec::new();
    let mut packets = 0;
    while let Some(mut event) = events.pop_front() {
        if matches!(event, InputEvent::MouseMove { .. }) {
            for _ in 1..4 {
                let Some(next) = events.front().copied() else {
                    break;
                };
                if !matches!(next, InputEvent::MouseMove { .. }) {
                    break;
                }
                let merged = if legacy {
                    match (event, next) {
                        (
                            InputEvent::MouseMove { dx, dy },
                            InputEvent::MouseMove { dx: x, dy: y },
                        ) => Some(InputEvent::MouseMove {
                            dx: dx.saturating_add(x),
                            dy: dy.saturating_add(y),
                        }),
                        _ => None,
                    }
                } else {
                    merge_motion(event, next)
                };
                match merged {
                    Some(next) => {
                        event = next;
                        events.pop_front();
                    }
                    None => break,
                }
            }
        }
        packets += 1;
        match event {
            InputEvent::MouseMove { dx, .. } => {
                distance += dx.clamp(-MAX_INJECT_DELTA_PX, MAX_INJECT_DELTA_PX) as i64
            }
            InputEvent::Key { down, .. } => edges.push((distance, down)),
            _ => {}
        }
    }
    (distance, edges, packets)
}

fn main() -> anyhow::Result<()> {
    let mode = std::env::args().nth(1).unwrap_or_else(|| "bounded".into());
    let legacy = mode == "legacy";
    for moves in [20, 50, 100, 160, 1_000] {
        let (tx, mut rx) = tokio::sync::broadcast::channel(64);
        let hub = InputHub::default();
        let mut sub = hub.subscribe();
        let mut events = VecDeque::new();
        let start = Instant::now();
        let mut dropped = 0;
        for i in 0..moves {
            let publish = |event| {
                if legacy {
                    let _ = tx.send(event);
                } else {
                    hub.send(event)
                }
            };
            publish(InputEvent::MouseMove { dx: 10, dy: 0 });
            if i == moves / 2 - 1 {
                publish(InputEvent::Key {
                    code: KeyCode(30),
                    down: true,
                });
                publish(InputEvent::Key {
                    code: KeyCode(30),
                    down: false,
                });
            }
        }
        if legacy {
            loop {
                match rx.try_recv() {
                    Ok(e) => events.push_back(e),
                    Err(tokio::sync::broadcast::error::TryRecvError::Lagged(n)) => dropped += n,
                    Err(_) => break,
                }
            }
        } else {
            while let Ok(e) = sub.try_recv() {
                events.push_back(e)
            }
        }
        let (distance, edges, packets) = consume(events, legacy);
        println!(
            "{}",
            serde_json::json!({"mode":mode,"moves":moves,"expected_distance":moves*10,"delivered_distance":distance,"expected_key_edges":2,"key_edges":edges,"broadcast_dropped":dropped,"packets":packets,"wall_us":start.elapsed().as_secs_f64()*1e6,"queue_diagnostics":sub.diagnostics()})
        );
        if !legacy {
            assert_eq!(distance, moves * 10);
            assert_eq!(edges, vec![(moves * 5, true), (moves * 5, false)]);
        }
    }
    let events = (0..4)
        .map(|_| InputEvent::MouseMove { dx: 150, dy: 0 })
        .collect();
    let (distance, _, _) = consume(events, legacy);
    println!(
        "{}",
        serde_json::json!({"mode":mode,"case":"four_legal_fast_moves","expected_distance":600,"delivered_distance":distance})
    );
    if !legacy {
        assert_eq!(distance, 600);
    }
    Ok(())
}
