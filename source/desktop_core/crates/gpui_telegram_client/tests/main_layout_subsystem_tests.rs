//! Main and Layout Subsystem Integration Tests (Client Domain)
//! Verifies client-side domain multi-account coordination, account slot limits, and headless session state.

use gpui_telegram_client::r#main::{
    ClientAccount, ClientDomain, ClientSession, CLIENT_MAX_ACCOUNTS, CLIENT_PREMIUM_MAX_ACCOUNTS,
};

#[test]
fn test_client_domain_account_limits_standard_and_premium() {
    let mut domain = ClientDomain::new();
    assert_eq!(domain.accounts.len(), 0);
    assert_eq!(CLIENT_MAX_ACCOUNTS, 3);
    assert_eq!(CLIENT_PREMIUM_MAX_ACCOUNTS, 6);

    // Standard user can add up to 3 accounts
    let acc0 = domain.add_account(false).expect("should add account 0");
    let acc1 = domain.add_account(false).expect("should add account 1");
    let acc2 = domain.add_account(false).expect("should add account 2");
    assert_eq!(acc0, 0);
    assert_eq!(acc1, 1);
    assert_eq!(acc2, 2);
    assert_eq!(domain.accounts.len(), 3);

    // 4th account fails for standard user
    assert!(domain.add_account(false).is_err());

    // Premium user can add up to 6 accounts
    let acc3 = domain.add_account(true).expect("should add account 3 with premium");
    let acc4 = domain.add_account(true).expect("should add account 4 with premium");
    let acc5 = domain.add_account(true).expect("should add account 5 with premium");
    assert_eq!(acc3, 3);
    assert_eq!(acc4, 4);
    assert_eq!(acc5, 5);
    assert_eq!(domain.accounts.len(), 6);

    // 7th account fails even for premium user
    assert!(domain.add_account(true).is_err());
}

#[test]
fn test_client_account_authentication_and_session_lifecycle() {
    let mut account = ClientAccount::new(0, "account_0".into());
    assert!(!account.is_authenticated());
    assert_eq!(account.main_dc_id, 2);

    // Authenticate
    let user_id = 11223344;
    account.authenticate(user_id, false);
    assert!(account.is_authenticated());

    let session = account.session.as_ref().unwrap();
    assert_eq!(session.user_id, user_id);
    assert!(!session.test_mode);
    assert!(!session.is_frozen());

    // Test mode authentication
    let mut test_account = ClientAccount::new(1, "account_1".into());
    test_account.authenticate(55667788, true);
    assert!(test_account.session.as_ref().unwrap().test_mode);

    // Logout
    account.logout();
    assert!(!account.is_authenticated());
}

#[test]
fn test_client_domain_active_account_switching() {
    let mut domain = ClientDomain::new();
    domain.add_account(false).unwrap();
    domain.add_account(false).unwrap();

    assert_eq!(domain.active_index, Some(0));
    assert_eq!(domain.active_account().unwrap().index, 0);

    // Switch to account 1
    assert!(domain.activate(1));
    assert_eq!(domain.active_account().unwrap().index, 1);

    // Switching to non-existent account fails
    assert!(!domain.activate(99));
    assert_eq!(domain.active_account().unwrap().index, 1);
}

#[test]
fn test_client_session_freeze_state() {
    let mut session = ClientSession::new(1001, false);
    assert!(!session.is_frozen());

    let now = chrono::Utc::now().timestamp();
    session.freeze_since = Some(now);
    session.appeal_url = Some("https://telegram.org/support".into());
    assert!(session.is_frozen());
    assert_eq!(session.appeal_url.as_deref(), Some("https://telegram.org/support"));
}
