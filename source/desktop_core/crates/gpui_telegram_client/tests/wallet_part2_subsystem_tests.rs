//! Module 13 Wallet Part 2 Subsystem Integration & Unit Tests
//! Tests for all 21 components and core modules in gpui_telegram_client::wallet

use gpui_telegram_client::wallet::components::*;
use gpui_telegram_client::wallet::core::*;

#[test]
fn test_ton_hash_and_crc() {
    let empty_sha = ton_hash::sha256(b"");
    assert_eq!(
        format!("{:02x}", empty_sha[0]),
        "e3",
        "SHA-256 standard leading byte for empty string is 0xe3"
    );

    let test_crc32 = ton_hash::crc32c(b"123456789");
    assert_eq!(test_crc32, 0xE3069283, "Castagnoli CRC32-C vector check");

    let test_crc16 = ton_hash::crc16_xmodem(b"123456789");
    assert_eq!(test_crc16, 0x31C3, "CRC16-XMODEM vector check");
}

#[test]
fn test_ton_crypto_and_keypair() {
    let seed = [42u8; 32];
    let keypair = TonKeyPair::from_secret_seed(&seed);
    assert_eq!(keypair.secret_key.len(), 32);
    assert_eq!(keypair.public_key.len(), 32);

    let msg = b"TON Transfer 10.5 TON to Alice";
    let sig = keypair.sign_message(msg);
    assert_eq!(sig.len(), 64);

    let mut random = [0u8; 16];
    ton_crypto::generate_random_bytes(&mut random);
    assert!(random.iter().any(|&b| b != 0));
}

#[test]
fn test_ton_bip39_and_mnemonic() {
    assert!(ton_bip39::is_valid_word("abandon"));
    assert!(ton_bip39::is_valid_word("zoo"));
    assert!(!ton_bip39::is_valid_word("definitelynotaword123"));

    let words: Vec<String> = (0..24).map(|_| "abandon".to_string()).collect();
    let mnemonic = TonMnemonic::from_words(words).expect("24 valid words must form mnemonic");
    let seed = mnemonic.to_seed("");
    assert_eq!(seed.len(), 64);
}

#[test]
fn test_ton_amount_logic() {
    let zero = TonAmount::ZERO;
    assert_eq!(zero.as_nanos(), 0);

    let five_ton = TonAmount::from_ton_f64(5.5).unwrap();
    assert_eq!(five_ton.as_nanos(), 5_500_000_000);
    assert_eq!(five_ton.format_ton(2), "5.5");
    assert_eq!(five_ton.format_ton(0), "5");

    let parsed_nano = TonAmount::from_nanos(123_456_789);
    assert_eq!(parsed_nano.format_ton(9), "0.123456789");
}

#[test]
fn test_ton_account_id_formatting() {
    let addr = [0xABu8; 32];
    let account = TonAccountId::new(0, addr);
    let raw = account.to_raw();
    assert!(raw.starts_with("0:abababab"));

    let friendly_bounceable = account.to_user_friendly(true, false);
    assert!(friendly_bounceable.starts_with("EQ"));
    assert_eq!(friendly_bounceable.len(), 48);

    let friendly_non_bounceable = account.to_user_friendly(false, false);
    assert!(friendly_non_bounceable.starts_with("UQ"));
    assert_eq!(friendly_non_bounceable.len(), 48);
}

#[test]
fn test_ton_cell_builder() {
    let mut root = TonCell::new();
    root.write_u32(0x12345678).expect("write u32 fits in cell");
    assert_eq!(root.bit_len, 32);

    let mut child = TonCell::new();
    child.write_bytes(&[1, 2, 3]).expect("write bytes fits in child");
    root.add_reference(child).expect("add reference fits in cell");
    assert_eq!(root.references.len(), 1);

    // Test max 4 references bound
    for _ in 0..3 {
        root.add_reference(TonCell::new()).unwrap();
    }
    assert!(root.add_reference(TonCell::new()).is_err());
}

#[test]
fn test_ton_address_and_amount_inputs() {
    let mut addr_input = TonAddressInput::new();
    assert_eq!(addr_input.validation_state, AddressValidationState::Empty);

    addr_input.set_text("EQAbabababababababababababababababababababababab");
    assert!(addr_input.is_valid());

    addr_input.set_text("not_valid");
    assert!(!addr_input.is_valid());

    let mut amount_input = TonAmountInput::new(10_000_000_000); // 10 TON
    assert!(amount_input.set_input("2.5"));
    assert_eq!(amount_input.amount_nano, 2_500_000_000);
    assert!(!amount_input.exceeds_balance());

    amount_input.apply_max();
    assert_eq!(amount_input.amount_nano, 10_000_000_000);
    assert_eq!(amount_input.raw_input, "10");
}

#[test]
fn test_ton_balance_card_and_fee_chip() {
    let mut card = TonBalanceCard::new(5_000_000_000, "EQAbabababababababababababababababababababababab");
    card.set_fiat_rate(6.0); // $6 / TON
    assert_eq!(card.formatted_ton(), "5 TON");
    assert_eq!(card.formatted_fiat(), "≈ $30.00");

    let mut chip = TonFeeChip::new();
    assert!(chip.is_estimating);
    chip.set_fee(15_000_000);
    assert_eq!(chip.display_text(), "Fee: ≈ 0.0150 TON");
}

#[test]
fn test_ton_keypad_and_comment_input() {
    let mut keypad = TonKeypad::new(true, true);
    keypad.press(KeypadKey::Digit(1));
    keypad.press(KeypadKey::Digit(2));
    keypad.press(KeypadKey::Dot);
    keypad.press(KeypadKey::Digit(5));
    assert_eq!(keypad.pressed_history.len(), 4);

    let mut comment = TonCommentInput::new();
    assert!(comment.set_text("Coffee payment"));
    assert_eq!(comment.remaining_chars(), 500 - 14);
    comment.toggle_encryption();
    assert!(comment.is_encrypted);
}

#[test]
fn test_ton_nft_token_selector_and_tabs() {
    let mut item = TonNftGridItem::new("0:12345", "Durov #1", "Telegram Numbers");
    assert!(!item.is_selected);
    item.toggle_select();
    assert!(item.is_selected);

    let mut tab = TonSegmentedTab::new(vec!["Assets".into(), "Activity".into(), "Collectibles".into()]);
    assert_eq!(tab.current_tab(), Some("Assets"));
    assert!(tab.select(1));
    assert_eq!(tab.current_tab(), Some("Activity"));

    let mut token_sel = TonTokenSelector::new();
    assert_eq!(token_sel.selected_symbol, "TON");
    token_sel.add_jetton(TokenItem {
        symbol: "USDT".into(),
        name: "Tether USD".into(),
        contract_address: Some("EQ_usdt".into()),
        balance_nano: 100_000_000,
        decimals: 6,
    });
    assert!(token_sel.select_token("USDT"));
    assert_eq!(token_sel.selected_symbol, "USDT");
}

#[test]
fn test_ton_transaction_row_and_app_bar() {
    let row = TonTransactionRow::new("tx_abc", TransactionDirection::Inbound, "EQ_sender", 2_500_000_000);
    assert_eq!(row.formatted_amount(), "+2.5000 TON");

    let mut app_bar = TonWalletAppBar::new("My Ton Wallet");
    assert_eq!(app_bar.network_label(), "Mainnet");
    app_bar.set_testnet(true);
    assert_eq!(app_bar.network_label(), "Testnet");
}
