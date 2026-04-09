use cosmwasm_schema::write_api;
use cosmwasm_std::Empty;
use factory::msg::ExecuteMsg;
use factory::query::QueryMsg;
use factory::state::FactoryInstantiate;

fn main() {
    write_api! {
        instantiate: FactoryInstantiate,
        execute: ExecuteMsg,
        query: QueryMsg,
        migrate: Empty,
    }
}
