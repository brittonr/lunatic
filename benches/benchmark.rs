use std::{collections::HashMap, sync::Arc};

use criterion::{Criterion, criterion_group, criterion_main};
// TODO: Re-export this under lunatic_runtime
use lunatic_process::{
    env::LunaticEnvironment,
    runtimes::wasmtime::{WasmtimeRuntime, default_config},
};
use lunatic_runtime::{DefaultProcessConfig, state::DefaultProcessState};
use tokio::sync::RwLock;

fn criterion_benchmark(c: &mut Criterion) {
    let rt = tokio::runtime::Runtime::new().unwrap();

    let config = Arc::new(DefaultProcessConfig::default());
    let wasmtime_config = default_config();
    let runtime = WasmtimeRuntime::new(&wasmtime_config).unwrap();

    let raw_module = wat::parse_file("./wat/hello.wat").unwrap();
    let module = Arc::new(
        runtime
            .compile_module::<DefaultProcessState>(raw_module.into())
            .unwrap(),
    );

    let env = Arc::new(LunaticEnvironment::new(0));
    c.bench_function("spawn process", |b| {
        b.to_async(&rt).iter(|| async {
            let registry = Arc::new(RwLock::new(HashMap::new()));
            let state = DefaultProcessState::new(
                env.clone(),
                None,
                runtime.clone(),
                module.clone(),
                config.clone(),
                registry,
                Default::default(),
            )
            .unwrap();
            lunatic_process::wasm::spawn_wasm(
                env.clone(),
                runtime.clone(),
                &module,
                state,
                "hello",
                Vec::new(),
                None,
            )
            .await
            .unwrap()
            .0
            .await
            .unwrap()
            .ok();
        });
    });
}

criterion_group!(benches, criterion_benchmark);
criterion_main!(benches);
