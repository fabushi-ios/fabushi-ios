//! Module 13 Wallet Part 4 Subsystem Integration & Unit Tests
//! Tests for all 20 screens and services in gpui_telegram_client::wallet

use gpui_telegram_client::wallet::screens::*;
use gpui_telegram_client::wallet::service::*;

#[test]
fn test_ton_history_screen_filtering() {
    let mut screen = TonHistoryScreen::new();
    let tx1 = TransactionHistoryEntry {
        tx_hash: "hash_1".into(),
        logical_time: 1000,
        timestamp: 1700000000,
        amount_nanotons: 5_000_000_000,
        fee_nanotons: 10_000_000,
        peer_address: "EQ_sender".into(),
        comment: Some("Gift".into()),
        is_jetton: false,
        token_symbol: None,
    };
    let tx2 = TransactionHistoryEntry {
        tx_hash: "hash_2".into(),
        logical_time: 2000,
        timestamp: 1700000500,
        amount_nanotons: -2_000_000_000,
        fee_nanotons: 8_000_000,
        peer_address: "EQ_recipient".into(),
        comment: None,
        is_jetton: false,
        token_symbol: None,
    };
    let tx3 = TransactionHistoryEntry {
        tx_hash: "hash_3".into(),
        logical_time: 3000,
        timestamp: 1700001000,
        amount_nanotons: 100_000_000,
        fee_nanotons: 50_000_000,
        peer_address: "EQ_jetton_pool".into(),
        comment: Some("Swap USDT".into()),
        is_jetton: true,
        token_symbol: Some("USDT".into()),
    };

    screen.append_transactions(vec![tx1, tx2, tx3], false);
    assert_eq!(screen.transactions.len(), 3);

    // Incoming filter
    screen.set_filter(HistoryFilter::Incoming);
    let incoming = screen.filtered_transactions();
    assert_eq!(incoming.len(), 2);

    // Outgoing filter
    screen.set_filter(HistoryFilter::Outgoing);
    let outgoing = screen.filtered_transactions();
    assert_eq!(outgoing.len(), 1);
    assert_eq!(outgoing[0].tx_hash, "hash_2");

    // Tokens filter
    screen.set_filter(HistoryFilter::Tokens);
    let tokens = screen.filtered_transactions();
    assert_eq!(tokens.len(), 1);
    assert_eq!(tokens[0].token_symbol.as_deref(), Some("USDT"));
}

#[test]
fn test_ton_import_wallet_screen() {
    let mut screen = TonImportWalletScreen::new(MnemonicWordCount::Words24);
    assert_eq!(screen.words.len(), 24);
    assert!(!screen.import_ready);

    let phrase = "word1 word2 word3 word4 word5 word6 word7 word8 word9 word10 word11 word12 \
                  word13 word14 word15 word16 word17 word18 word19 word20 word21 word22 word23 word24";
    let ok = screen.paste_phrase(phrase);
    assert!(ok);
    assert!(screen.import_ready);
    assert_eq!(screen.words[0], "word1");
    assert_eq!(screen.words[23], "word24");
}

#[test]
fn test_ton_main_wallet_screen() {
    let mut screen = TonMainWalletScreen::new("EQ_bounceable_address", "0:raw_hex_address");
    assert_eq!(screen.address_bounceable, "EQ_bounceable_address");

    screen.set_balance(10_000_000_000, 5.50); // 10 TON @ $5.50
    assert_eq!(screen.total_balance_nanotons, 10_000_000_000);
    assert!((screen.total_fiat_value - 55.0).abs() < 1e-4);

    screen.add_asset(AssetItem {
        symbol: "USDT".into(),
        name: "Tether USD".into(),
        balance_nanos: 25_000_000,
        decimals: 6,
        fiat_value: 25.0,
        icon_url: None,
    });
    screen.set_balance(10_000_000_000, 5.50);
    assert!((screen.total_fiat_value - 80.0).abs() < 1e-4);
}

#[test]
fn test_ton_nft_detail_screen() {
    let mut screen = TonNftDetailScreen::new("EQ_nft_item", "Telegram Username #1234");
    screen.set_collection("EQ_collection", "Usernames");
    screen.add_attribute("Length", "4");
    assert_eq!(screen.attributes.len(), 1);
    assert!(screen.start_transfer());
    assert!(!screen.start_transfer()); // Already in progress
}

#[test]
fn test_ton_passcode_screen() {
    let mut screen = TonPasscodeScreen::new(PasscodeScreenMode::Unlock);
    assert!(screen.enter_digit('1'));
    assert!(screen.enter_digit('2'));
    assert!(screen.enter_digit('3'));
    screen.delete_digit();
    assert_eq!(screen.entered_digits, "12");
    assert!(!screen.enter_digit('a')); // Non-digit rejected

    screen.set_lockout(30);
    assert!(!screen.enter_digit('5')); // Lockout blocks entry
}

#[test]
fn test_ton_qr_receive_screen() {
    let mut screen = TonQrReceiveScreen::new("EQ_test_recipient_address");
    assert_eq!(screen.qr_data_uri, "ton://transfer/EQ_test_recipient_address");

    screen.set_amount(Some(5_000_000_000));
    screen.set_comment(Some("Dinner".into()));
    assert_eq!(
        screen.qr_data_uri,
        "ton://transfer/EQ_test_recipient_address?amount=5000000000&text=Dinner"
    );
    screen.mark_copied();
    assert!(screen.copied_to_clipboard);
}

#[test]
fn test_ton_receive_screen() {
    let mut screen = TonReceiveScreen::new("EQ_bounceable", "UQ_non_bounceable");
    assert_eq!(screen.active_address(), "EQ_bounceable");
    screen.toggle_bounceable();
    assert_eq!(screen.active_address(), "UQ_non_bounceable");
    screen.mark_copied();
    assert!(screen.copied);
}

#[test]
fn test_ton_send_amount_screen() {
    let mut screen = TonSendAmountScreen::new("TON", 10_000_000_000, 5.0);
    screen.set_percentage(50);
    assert!(screen.is_valid);
    assert!((screen.fiat_equivalent - 25.0).abs() < 1e-2);

    screen.set_amount_text("20.0"); // More than 10 TON balance
    assert!(!screen.is_valid);
    assert_eq!(screen.validation_error.as_deref(), Some("Insufficient balance"));
}

#[test]
fn test_ton_send_recipient_screen() {
    let mut screen = TonSendRecipientScreen::new();
    screen.set_input("alice.ton");
    assert!(screen.is_resolving_dns);
    assert!(!screen.is_valid);

    screen.on_dns_resolved("EQB_valid_ton_wallet_address_48_chars_long_12345");
    assert!(screen.is_valid);
    assert_eq!(
        screen.resolved_address.as_deref(),
        Some("EQB_valid_ton_wallet_address_48_chars_long_12345")
    );
}

#[test]
fn test_ton_send_screen() {
    let mut screen = TonSendScreen::new("EQ_destination", 5_000_000_000);
    assert_eq!(screen.status, SendStatus::Review);
    assert_eq!(screen.total_nanotons(), 5_008_000_000);

    screen.set_comment("Secret gift", true);
    assert!(screen.encrypt_comment);

    screen.confirm_send();
    assert_eq!(screen.status, SendStatus::Signing);
    screen.mark_broadcasting();
    assert_eq!(screen.status, SendStatus::Broadcasting);
    screen.on_success("tx_hash_ton_blockchain_123");
    assert_eq!(screen.status, SendStatus::Success);
    assert_eq!(screen.transaction_hash.as_deref(), Some("tx_hash_ton_blockchain_123"));
}

#[test]
fn test_ton_settings_screen() {
    let mut screen = TonSettingsScreen::new();
    assert_eq!(screen.active_currency, "USD");
    screen.set_currency("EUR");
    assert_eq!(screen.active_currency, "EUR");
    screen.set_network(TonNetwork::Testnet);
    assert_eq!(screen.network, TonNetwork::Testnet);
}

#[test]
fn test_ton_staking_screen() {
    let mut screen = TonStakingScreen::new();
    assert!(screen.active_pool.is_some());
    screen.select_pool("Tonstakers Liquid Staking");
    assert_eq!(
        screen.active_pool.as_ref().map(|p| p.name.as_str()),
        Some("Tonstakers Liquid Staking")
    );
    screen.update_stake(100_000_000_000, 250_000_000, 1720000000);
    assert_eq!(screen.staked_balance_nanotons, 100_000_000_000);
}

#[test]
fn test_ton_swap_screen() {
    let mut screen = TonSwapScreen::new();
    assert_eq!(screen.from_token.symbol, "TON");
    assert_eq!(screen.to_token.symbol, "USDT");
    screen.flip_tokens();
    assert_eq!(screen.from_token.symbol, "USDT");
    assert_eq!(screen.to_token.symbol, "TON");
    screen.set_slippage(2.5);
    assert_eq!(screen.slippage_tolerance_pct, 2.5);
}

#[test]
fn test_ton_token_detail_screen() {
    let mut screen = TonTokenDetailScreen::new("NOT", "Notcoin", "EQ_notcoin_master");
    screen.balance_nanos = 100_000_000_000;
    screen.decimals = 9;
    screen.set_price_data(0.008, 5.2, vec![0.0075, 0.0078, 0.008]);
    assert!((screen.fiat_value() - 0.8).abs() < 1e-4);
    assert_eq!(screen.formatted_balance(), "100.0000 NOT");
}

#[test]
fn test_ton_wallet_settings_screen() {
    let mut screen = TonWalletSettingsScreen::new("My Wallet", "EQ_my_addr");
    assert!(screen.biometrics_enabled);
    let disabled = screen.toggle_biometrics();
    assert!(!disabled);
    screen.set_auto_lock(15);
    assert_eq!(screen.auto_lock_timeout_minutes, 15);
    screen.prompt_delete();
    assert!(screen.is_delete_dialog_open);
    screen.cancel_delete();
    assert!(!screen.is_delete_dialog_open);
}

#[tokio::test]
async fn test_ton_biometrics_service() {
    let service = TonBiometricsService::new();
    if service.can_authenticate() {
        let auth = service.authenticate("Unlock Wallet").await;
        assert!(auth.is_ok());
    }
}

#[test]
fn test_ton_keychain_service() {
    let keychain = TonKeychainService::new("test_service");
    let secret = b"my_super_secret_mnemonic_vault_key_32_bytes";
    assert!(keychain.store_secret("vault_key", secret).is_ok());
    assert!(keychain.has_secret("vault_key"));

    let loaded = keychain.retrieve_secret("vault_key").unwrap();
    assert_eq!(loaded, secret);

    assert!(keychain.delete_secret("vault_key").is_ok());
    assert!(!keychain.has_secret("vault_key"));
}

#[test]
fn test_ton_price_service() {
    let price_service = TonPriceService::new();
    let ton_price = price_service.get_token_price("TON");
    assert!(ton_price.is_some());
    assert_eq!(ton_price.unwrap(), 5.50);

    let eur_converted = price_service.convert_to_fiat(100.0, "EUR");
    assert!((eur_converted - 92.0).abs() < 1e-4);

    price_service.update_quote("TON", 6.0, 9.1, 1720000000);
    assert_eq!(price_service.get_token_price("TON"), Some(6.0));
}

#[test]
fn test_ton_secure_storage() {
    let storage = TonSecureStorage::new();
    let account = StoredWalletAccount {
        account_id: "acc_1".into(),
        address: "EQ_address_1".into(),
        label: "Main Account".into(),
        is_active: true,
        created_at_secs: 1720000000,
    };

    storage.save_account(account);
    assert_eq!(storage.list_accounts().len(), 1);
    assert!(storage.active_account().is_some());

    storage.store_kv("pref_theme", b"dark");
    assert_eq!(storage.get_kv("pref_theme").as_deref(), Some(&b"dark"[..]));
}

#[tokio::test]
async fn test_ton_wallet_service_lifecycle() {
    let wallet_svc = TonWalletService::new();
    let acc = wallet_svc
        .create_new_wallet("acc_main", "EQ_main_wallet_addr", "Personal", 1720000000)
        .await
        .unwrap();

    assert_eq!(acc.account_id, "acc_main");
    assert!(wallet_svc.get_active_account().is_some());

    wallet_svc.set_balance_nanotons(10_000_000_000); // 10 TON
    assert_eq!(wallet_svc.get_cached_balance_nanotons(), 10_000_000_000);

    // Successful transfer
    let tx = wallet_svc
        .send_transfer("EQ_recipient", 2_000_000_000, 8_000_000)
        .await;
    assert!(tx.is_ok());
    assert_eq!(
        wallet_svc.get_cached_balance_nanotons(),
        10_000_000_000 - 2_008_000_000
    );

    // Overdraft transfer error
    let overdraft = wallet_svc
        .send_transfer("EQ_recipient", 50_000_000_000, 8_000_000)
        .await;
    assert!(overdraft.is_err());
}
