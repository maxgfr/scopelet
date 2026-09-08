use crate::{
    model::{Format, Operation},
    pipeline, sources,
    store::Store,
};
use anyhow::{Result, ensure};
use serde_json::json;

pub fn offline() -> Result<serde_json::Value> {
    let temp = tempfile::tempdir()?;
    let store = Store::open(Some(temp.path().join("cache")))?;
    let raw = (0..1000).map(|i| json!({"id":i,"status":if i%10==0 {"failed"} else {"passed"},"suite":i%4,"payload":"unchanged fixture text"}).to_string()).collect::<Vec<_>>().join("\n");
    let mut data = crate::model::Dataset::default();
    sources::ingest(
        &mut data,
        &store,
        "fixture",
        raw.as_bytes().to_vec(),
        None,
        Format::Jsonl,
    )?;
    pipeline::apply(
        &mut data,
        &[
            Operation::Filter {
                pointer: "/status".into(),
                equals: json!("failed"),
            },
            Operation::Count,
        ],
    )?;
    ensure!(
        data.records[0].value == Some(json!({"count":100})),
        "offline correctness gate failed"
    );
    let view = crate::render::render(&data, &store, crate::model::Mode::Default, 16384, 0)?;
    Ok(
        json!({"passed":true,"fixture":"1000 JSONL records; exact failed count","input_bytes":raw.len(),"output_bytes":serde_json::to_vec(&view)?.len(),"count":100,"measurement":"bytes only; not model tokens or session savings"}),
    )
}
