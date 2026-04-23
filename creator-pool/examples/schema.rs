use cosmwasm_schema::write_api;
use creator_pool::msg::{ExecuteMsg, MigrateMsg, PoolInstantiateMsg, QueryMsg};

fn main() {
    write_api! {
        instantiate: PoolInstantiateMsg,
        execute: ExecuteMsg,
        query: QueryMsg,
        migrate: MigrateMsg,
    }
}
