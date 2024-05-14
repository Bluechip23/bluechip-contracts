use std::env::current_dir;
use std::fs::create_dir_all;

use cosmwasm_schema::{export_schema, remove_schemas, schema_for};
use factory::msg::{ConfigResponse, ExecuteMsg, InstantiateMsg, QueryMsg};
use factory::state::Config;

fn main() {
    let mut out_dir = current_dir().unwrap();

    // out_dir.push("contracts/price_prediction/schema");
    out_dir.push("schema");

    println!("creating dir at {:?}", out_dir.display());
    create_dir_all(&out_dir).unwrap();
    remove_schemas(&out_dir).unwrap();

    export_schema(&schema_for!(InstantiateMsg), &out_dir);
    export_schema(&schema_for!(ExecuteMsg), &out_dir);
    export_schema(&schema_for!(QueryMsg), &out_dir);
    export_schema(&schema_for!(Config), &out_dir);
    export_schema(&schema_for!(ConfigResponse), &out_dir);
}
