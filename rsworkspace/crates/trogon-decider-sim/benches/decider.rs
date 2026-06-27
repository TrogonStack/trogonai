#![allow(clippy::expect_used, clippy::panic, clippy::unwrap_used)]

use std::hint::black_box;

use buffa::Message as _;
use buffa::MessageName as _;
use criterion::{Criterion, criterion_group, criterion_main};
use trogon_decider_sim::{SimFixture, SimHost, SimScenario};
use trogon_decider_wit::host::{self, CommandEnvelope};
use trogonai_proto::example::{TURN_ON_TYPE_URL, v1};

fn turn_on_command(light_id: &str) -> CommandEnvelope {
    CommandEnvelope {
        type_: TURN_ON_TYPE_URL.to_string(),
        payload: v1::TurnOn {
            light_id: light_id.to_string(),
        }
        .encode_to_vec(),
    }
}

fn light_turned_on_event(light_id: &str, count: u64) -> host::AnyEnvelope {
    host::AnyEnvelope {
        type_: v1::LightTurnedOn::FULL_NAME.to_string(),
        payload: v1::LightTurnedOn {
            light_id: light_id.to_string(),
            turn_on_count: count,
        }
        .encode_to_vec(),
    }
}

fn bench_decider(c: &mut Criterion) {
    let wasm = SimFixture::light().bytes().to_vec();
    let host = SimHost::load(&wasm).expect("load light fixture");

    for event_count in [1usize, 10, 100, 10_000] {
        c.bench_function(&format!("evolve/{event_count}"), |b| {
            b.iter(|| {
                let mut instance = host.instantiate(()).expect("instantiate");
                let given = (1..=event_count)
                    .map(|count| light_turned_on_event("bench", count as u64))
                    .collect::<Vec<_>>();
                SimScenario::new()
                    .given(given)
                    .when(turn_on_command("bench"))
                    .then_rejected()
                    .run(&mut instance)
                    .expect("scenario");
                black_box(&instance);
            });
        });
    }

    c.bench_function("decide/initial", |b| {
        b.iter(|| {
            let mut instance = host.instantiate(()).expect("instantiate");
            SimScenario::new()
                .when(turn_on_command("bench"))
                .then_events([light_turned_on_event("bench", 1)])
                .run(&mut instance)
                .expect("scenario");
            black_box(&instance);
        });
    });
}

criterion_group!(benches, bench_decider);
criterion_main!(benches);
