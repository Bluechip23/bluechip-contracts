
#[cfg(test)]
mod tests {
    use super::*;
    use cosmwasm_std::testing::mock_info;
    use cosmwasm_std::{coin, coins};

    #[test]
    fn test_native_coins_sent() {
        let asset = native_asset_info("uusd".to_string()).with_balance(1000u16);
        let addr = Addr::unchecked("addr0000");

        let info = mock_info(&addr.as_str(), &coins(1000, "random"));
        let err = asset.assert_sent_native_token_balance(&info).unwrap_err();
        assert_eq!(err, StdError::generic_err("Must send reserve token 'uusd'"));

        let info = mock_info(&addr.as_str(), &coins(100, "uusd"));
        let err = asset.assert_sent_native_token_balance(&info).unwrap_err();
        assert_eq!(
            err,
            StdError::generic_err(
                "Native token balance mismatch between the argument and the transferred"
            )
        );

        let info = mock_info(&addr.as_str(), &coins(1000, "uusd"));
        asset.assert_sent_native_token_balance(&info).unwrap();
    }

    #[test]
    fn test_proper_native_coins_sent() {
        let pool_asset_infos = [
            native_asset_info("uusd".to_string()),
            native_asset_info("uluna".to_string()),
        ];

        let assets = [
            pool_asset_infos[0].with_balance(1000u16),
            pool_asset_infos[1].with_balance(100u16),
        ];
        let err = vec![coin(1000, "uusd"), coin(1000, "random")]
            .assert_coins_properly_sent(&assets, &pool_asset_infos)
            .unwrap_err();
        assert_eq!(
            err,
            StdError::generic_err(
                "Supplied coins contain random that is not in the input asset vector"
            )
        );

        let assets = [
            pool_asset_infos[0].with_balance(1000u16),
            native_asset_info("random".to_string()).with_balance(100u16),
        ];
        let err = vec![coin(1000, "uusd"), coin(100, "random")]
            .assert_coins_properly_sent(&assets, &pool_asset_infos)
            .unwrap_err();
        assert_eq!(
            err,
            StdError::generic_err("Asset random is not in the pool")
        );

        let assets = [
            pool_asset_infos[0].with_balance(1000u16),
            pool_asset_infos[1].with_balance(1000u16),
        ];
        let err = vec![coin(1000, "uusd"), coin(100, "uluna")]
            .assert_coins_properly_sent(&assets, &pool_asset_infos)
            .unwrap_err();
        assert_eq!(
            err,
            StdError::generic_err(
                "Native token balance mismatch between the argument and the transferred"
            )
        );

        let assets = [
            pool_asset_infos[0].with_balance(1000u16),
            pool_asset_infos[1].with_balance(1000u16),
        ];
        vec![coin(1000, "uusd"), coin(1000, "uluna")]
            .assert_coins_properly_sent(&assets, &pool_asset_infos)
            .unwrap();

        let pool_asset_infos = [
            token_asset_info(Addr::unchecked("addr0000")),
            token_asset_info(Addr::unchecked("addr0001")),
        ];
        let assets = [
            pool_asset_infos[0].with_balance(1000u16),
            pool_asset_infos[1].with_balance(1000u16),
        ];
        let err = vec![coin(1000, "uusd"), coin(1000, "uluna")]
            .assert_coins_properly_sent(&assets, &pool_asset_infos)
            .unwrap_err();
        assert_eq!(
            err,
            StdError::generic_err(
                "Supplied coins contain uusd that is not in the input asset vector"
            )
        );
    }
}
