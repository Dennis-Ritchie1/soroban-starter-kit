#![cfg(test)]

use super::*;
use soroban_sdk::{
    testutils::{Address as _, Ledger as _},
    token::TokenInterface,
    Address, Env,
};

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn setup_escrow<'a>(
    env: &'a Env,
    buyer: &Address,
    seller: &Address,
    arbiter: &Address,
    amount: i128,
) -> (EscrowContractClient<'a>, Address) {
    let token_addr = env.register_contract(None, MockToken);
    let escrow_addr = env.register_contract(None, EscrowContract);
    let client = EscrowContractClient::new(env, &escrow_addr);
    let deadline = env.ledger().sequence() + 200;
    client.initialize(buyer, seller, arbiter, &token_addr, &amount, &deadline);
    (client, token_addr)
}

fn setup_funded_escrow(
    env: &Env,
) -> (
    EscrowContractClient<'_>,
    Address,
    Address,
    Address,
    Address,
    i128,
    u32,
) {
    let buyer = Address::generate(env);
    let seller = Address::generate(env);
    let arbiter = Address::generate(env);
    let amount = 1_000i128;
    let (client, token_addr) = setup_escrow(env, &buyer, &seller, &arbiter, amount);
    let deadline = env.ledger().sequence() + 200;
    client.fund();
    (client, buyer, seller, arbiter, token_addr, amount, deadline)
}

// ---------------------------------------------------------------------------
// MockToken — no-op token so cross-contract calls don't panic
// ---------------------------------------------------------------------------

#[contract]
pub struct MockToken;

#[contractimpl]
impl token::TokenInterface for MockToken {
    fn allowance(_env: Env, _from: Address, _spender: Address) -> i128 {
        0
    }
    fn approve(_env: Env, _from: Address, _spender: Address, _amount: i128, _exp: u32) {}
    fn balance(_env: Env, _id: Address) -> i128 {
        i128::MAX
    }
    fn transfer(_env: Env, _from: Address, _to: Address, _amount: i128) {}
    fn transfer_from(_env: Env, _spender: Address, _from: Address, _to: Address, _amount: i128) {}
    fn burn(_env: Env, _from: Address, _amount: i128) {}
    fn burn_from(_env: Env, _spender: Address, _from: Address, _amount: i128) {}
    fn decimals(_env: Env) -> u32 {
        18
    }
    fn name(env: Env) -> soroban_sdk::String {
        soroban_sdk::String::from_str(&env, "Mock")
    }
    fn symbol(env: Env) -> soroban_sdk::String {
        soroban_sdk::String::from_str(&env, "MCK")
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[test]
fn test_get_state_none_before_init() {
    let env = Env::default();
    env.mock_all_auths();
    let addr = env.register_contract(None, EscrowContract);
    let client = EscrowContractClient::new(&env, &addr);
    assert_eq!(client.get_state(), None);
}

#[test]
fn test_initialize() {
    let env = Env::default();
    env.mock_all_auths();
    let buyer = Address::generate(&env);
    let seller = Address::generate(&env);
    let arbiter = Address::generate(&env);
    let (client, _) = setup_escrow(&env, &buyer, &seller, &arbiter, 1_000);
    assert_eq!(client.get_state(), Some(EscrowState::Created));
}

#[test]
#[should_panic(expected = "Error(Contract, #5)")]
fn test_initialize_twice() {
    let env = Env::default();
    env.mock_all_auths();
    let buyer = Address::generate(&env);
    let seller = Address::generate(&env);
    let arbiter = Address::generate(&env);
    let token_addr = env.register_contract(None, MockToken);
    let escrow_addr = env.register_contract(None, EscrowContract);
    let client = EscrowContractClient::new(&env, &escrow_addr);
    let deadline = env.ledger().sequence() + 200;
    client.initialize(&buyer, &seller, &arbiter, &token_addr, &1_000, &deadline);
    client.initialize(&buyer, &seller, &arbiter, &token_addr, &1_000, &deadline);
}

#[test]
#[should_panic]
fn test_initialize_past_deadline() {
    let env = Env::default();
    env.mock_all_auths();
    env.ledger().with_mut(|l| l.sequence_number = 200);
    let buyer = Address::generate(&env);
    let seller = Address::generate(&env);
    let arbiter = Address::generate(&env);
    let token_addr = env.register_contract(None, MockToken);
    let escrow_addr = env.register_contract(None, EscrowContract);
    let client = EscrowContractClient::new(&env, &escrow_addr);
    let deadline = 5u32; // well below sequence 200
    client.initialize(&buyer, &seller, &arbiter, &token_addr, &1_000, &deadline);
}

#[test]
fn test_fund() {
    let env = Env::default();
    env.mock_all_auths();
    let buyer = Address::generate(&env);
    let seller = Address::generate(&env);
    let arbiter = Address::generate(&env);
    let (client, _) = setup_escrow(&env, &buyer, &seller, &arbiter, 1_000);
    client.fund();
    assert_eq!(client.get_state(), Some(EscrowState::Funded));
}

#[test]
fn test_mark_delivered() {
    let env = Env::default();
    env.mock_all_auths();
    let (client, ..) = setup_funded_escrow(&env);
    client.mark_delivered();
    assert_eq!(client.get_state(), Some(EscrowState::Delivered));
}

#[test]
fn test_approve_delivery() {
    let env = Env::default();
    env.mock_all_auths();
    let (client, ..) = setup_funded_escrow(&env);
    client.mark_delivered();
    client.approve_delivery();
    assert_eq!(client.get_state(), Some(EscrowState::Completed));
}

#[test]
fn test_raise_dispute() {
    let env = Env::default();
    env.mock_all_auths();
    let (client, ..) = setup_funded_escrow(&env);
    client.raise_dispute();
    assert_eq!(client.get_state(), Some(EscrowState::Disputed));
}

#[test]
fn test_resolve_dispute_to_seller() {
    let env = Env::default();
    env.mock_all_auths();
    let (client, ..) = setup_funded_escrow(&env);
    client.raise_dispute();
    client.resolve_dispute(&true);
    assert_eq!(client.get_state(), Some(EscrowState::Completed));
}

#[test]
fn test_resolve_dispute_to_buyer() {
    let env = Env::default();
    env.mock_all_auths();
    let (client, ..) = setup_funded_escrow(&env);
    client.raise_dispute();
    client.resolve_dispute(&false);
    assert_eq!(client.get_state(), Some(EscrowState::Refunded));
}

#[test]
fn test_is_deadline_passed() {
    let env = Env::default();
    env.mock_all_auths();
    let buyer = Address::generate(&env);
    let seller = Address::generate(&env);
    let arbiter = Address::generate(&env);
    let token_addr = env.register_contract(None, MockToken);
    let escrow_addr = env.register_contract(None, EscrowContract);
    let client = EscrowContractClient::new(&env, &escrow_addr);
    let deadline = env.ledger().sequence() + 100; // exactly MIN_DEADLINE_BUFFER
    client.initialize(&buyer, &seller, &arbiter, &token_addr, &1_000, &deadline);
    assert!(!client.is_deadline_passed());
    env.ledger().with_mut(|l| l.sequence_number = deadline + 1);
    assert!(client.is_deadline_passed());
}

#[test]
#[should_panic(expected = "Error(Contract, #2)")]
fn test_invalid_mark_delivered_from_created() {
    let env = Env::default();
    env.mock_all_auths();
    let buyer = Address::generate(&env);
    let seller = Address::generate(&env);
    let arbiter = Address::generate(&env);
    let (client, _) = setup_escrow(&env, &buyer, &seller, &arbiter, 1_000);
    // Not funded — mark_delivered must fail with InvalidState (#2)
    client.mark_delivered();
}

#[test]
#[should_panic(expected = "Error(Contract, #8)")]
fn test_initialize_zero_amount() {
    let env = Env::default();
    env.mock_all_auths();
    let buyer = Address::generate(&env);
    let seller = Address::generate(&env);
    let arbiter = Address::generate(&env);
    let token_addr = env.register_contract(None, MockToken);
    let escrow_addr = env.register_contract(None, EscrowContract);
    let client = EscrowContractClient::new(&env, &escrow_addr);
    let deadline = env.ledger().sequence() + 200;
    client.initialize(&buyer, &seller, &arbiter, &token_addr, &0, &deadline);
}

#[test]
#[should_panic(expected = "Error(Contract, #8)")]
fn test_initialize_negative_amount() {
    let env = Env::default();
    env.mock_all_auths();
    let buyer = Address::generate(&env);
    let seller = Address::generate(&env);
    let arbiter = Address::generate(&env);
    let token_addr = env.register_contract(None, MockToken);
    let escrow_addr = env.register_contract(None, EscrowContract);
    let client = EscrowContractClient::new(&env, &escrow_addr);
    let deadline = env.ledger().sequence() + 200;
    client.initialize(&buyer, &seller, &arbiter, &token_addr, &-1, &deadline);
}
