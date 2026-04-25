use cosmwasm_schema::write_api;
use standard_pool::msg::{ExecuteMsg, MigrateMsg, QueryMsg};
use pool_factory_interfaces::StandardPoolInstantiateMsg;

fn main() {
    write_api! {
        instantiate: StandardPoolInstantiateMsg,
        execute: ExecuteMsg,
        query: QueryMsg,
        migrate: MigrateMsg,
    }
}
