#![no_std]

use soroban_sdk::{contract, contractimpl, token, Address, Env, Symbol};

mod admin;
mod errors;
mod storage;

use admin::require_admin;
use errors::EscrowError;
use storage::{DataKey, EscrowInfo, EscrowState};

const LEDGER_LIFETIME_THRESHOLD: u32 = 120_960;
const LEDGER_BUMP_AMOUNT: u32 = 518_400;
const MIN_DEADLINE_BUFFER: u32 = 100;

fn bump_instance(env: &Env) {
    env.storage()
        .instance()
        .extend_ttl(LEDGER_LIFETIME_THRESHOLD, LEDGER_BUMP_AMOUNT);
}

#[contract]
pub struct EscrowContract;

#[contractimpl]
impl EscrowContract {
    /// Initialize a new escrow. Must be called exactly once.
    pub fn initialize(
        env: Env,
        buyer: Address,
        seller: Address,
        arbiter: Address,
        token_contract: Address,
        amount: i128,
        deadline_ledger: u32,
    ) -> Result<(), EscrowError> {
        if env.storage().instance().has(&DataKey::State) {
            return Err(EscrowError::AlreadyInitialized);
        }
        if amount <= 0 {
            return Err(EscrowError::InvalidAmount);
        }
        if buyer == seller || buyer == arbiter || seller == arbiter {
            return Err(EscrowError::InvalidParties);
        }
        if deadline_ledger < env.ledger().sequence() + MIN_DEADLINE_BUFFER {
            return Err(EscrowError::DeadlinePassed);
        }

        // Validate token contract by calling decimals()
        let _ = token::Client::new(&env, &token_contract).decimals();

        env.storage().instance().set(&DataKey::Buyer, &buyer);
        env.storage().instance().set(&DataKey::Seller, &seller);
        env.storage().instance().set(&DataKey::Arbiter, &arbiter);
        env.storage().instance().set(&DataKey::TokenContract, &token_contract);
        env.storage().instance().set(&DataKey::Amount, &amount);
        env.storage().instance().set(&DataKey::Deadline, &deadline_ledger);
        env.storage().instance().set(&DataKey::State, &EscrowState::Created);
        env.storage().instance().set(&DataKey::BuyerApproved, &false);
        env.storage().instance().set(&DataKey::SellerDelivered, &false);
        bump_instance(&env);

        env.events().publish(
            (Symbol::new(&env, "escrow_created"), buyer.clone(), seller.clone()),
            amount,
        );
        Ok(())
    }

    /// Buyer funds the escrow. Escrow must be in `Created` state.
    pub fn fund(env: Env) -> Result<(), EscrowError> {
        let buyer: Address = env
            .storage()
            .instance()
            .get(&DataKey::Buyer)
            .ok_or(EscrowError::NotInitialized)?;
        buyer.require_auth();

        let state: EscrowState = env
            .storage()
            .instance()
            .get(&DataKey::State)
            .ok_or(EscrowError::NotInitialized)?;
        if state != EscrowState::Created {
            return Err(EscrowError::InvalidState);
        }

        let token_contract: Address = env.storage().instance().get(&DataKey::TokenContract).unwrap();
        let amount: i128 = env.storage().instance().get(&DataKey::Amount).unwrap();

        token::Client::new(&env, &token_contract)
            .transfer(&buyer, &env.current_contract_address(), &amount);

        env.storage().instance().set(&DataKey::State, &EscrowState::Funded);
        bump_instance(&env);

        env.events()
            .publish((Symbol::new(&env, "escrow_funded"), buyer), amount);
        Ok(())
    }

    /// Seller marks goods/services as delivered. Escrow must be in `Funded` state.
    pub fn mark_delivered(env: Env) -> Result<(), EscrowError> {
        let seller: Address = env
            .storage()
            .instance()
            .get(&DataKey::Seller)
            .ok_or(EscrowError::NotInitialized)?;
        seller.require_auth();

        let state: EscrowState = env
            .storage()
            .instance()
            .get(&DataKey::State)
            .ok_or(EscrowError::NotInitialized)?;
        if state != EscrowState::Funded {
            return Err(EscrowError::InvalidState);
        }

        env.storage().instance().set(&DataKey::SellerDelivered, &true);
        env.storage().instance().set(&DataKey::State, &EscrowState::Delivered);
        bump_instance(&env);

        env.events()
            .publish((Symbol::new(&env, "delivery_marked"), seller), ());
        Ok(())
    }

    /// Buyer approves delivery, releasing funds to the seller.
    pub fn approve_delivery(env: Env) -> Result<(), EscrowError> {
        let buyer: Address = env
            .storage()
            .instance()
            .get(&DataKey::Buyer)
            .ok_or(EscrowError::NotInitialized)?;
        buyer.require_auth();

        let state: EscrowState = env
            .storage()
            .instance()
            .get(&DataKey::State)
            .ok_or(EscrowError::NotInitialized)?;
        if state != EscrowState::Delivered {
            return Err(EscrowError::InvalidState);
        }

        Self::release_to_seller(env)
    }

    /// Buyer requests a refund after the deadline has passed.
    pub fn request_refund(env: Env) -> Result<(), EscrowError> {
        let buyer: Address = env
            .storage()
            .instance()
            .get(&DataKey::Buyer)
            .ok_or(EscrowError::NotInitialized)?;
        buyer.require_auth();

        let state: EscrowState = env
            .storage()
            .instance()
            .get(&DataKey::State)
            .ok_or(EscrowError::NotInitialized)?;
        let deadline: u32 = env
            .storage()
            .instance()
            .get(&DataKey::Deadline)
            .ok_or(EscrowError::NotInitialized)?;

        let can_refund = matches!(state, EscrowState::Funded | EscrowState::Delivered)
            && env.ledger().sequence() > deadline;
        if !can_refund {
            return Err(EscrowError::DeadlineNotReached);
        }

        Self::refund_to_buyer(env)
    }

    /// Buyer or seller raises a dispute. Escrow must be in `Funded` or `Delivered` state.
    pub fn raise_dispute(env: Env) -> Result<(), EscrowError> {
        let state: EscrowState = env
            .storage()
            .instance()
            .get(&DataKey::State)
            .ok_or(EscrowError::NotInitialized)?;
        if !matches!(state, EscrowState::Funded | EscrowState::Delivered) {
            return Err(EscrowError::InvalidState);
        }

        let buyer: Address = env.storage().instance().get(&DataKey::Buyer).unwrap();
        buyer.require_auth();

        env.storage().instance().set(&DataKey::State, &EscrowState::Disputed);
        bump_instance(&env);

        env.events()
            .publish((Symbol::new(&env, "dispute_raised"), buyer), ());
        Ok(())
    }

    /// Arbiter resolves a dispute. Escrow must be in `Disputed` state.
    pub fn resolve_dispute(env: Env, release_to_seller: bool) -> Result<(), EscrowError> {
        let arbiter: Address = env
            .storage()
            .instance()
            .get(&DataKey::Arbiter)
            .ok_or(EscrowError::NotInitialized)?;
        arbiter.require_auth();

        let state: EscrowState = env
            .storage()
            .instance()
            .get(&DataKey::State)
            .ok_or(EscrowError::NotInitialized)?;
        if state != EscrowState::Disputed {
            return Err(EscrowError::InvalidState);
        }

        if release_to_seller {
            Self::release_to_seller(env)
        } else {
            Self::refund_to_buyer(env)
        }
    }

    /// Buyer partially releases `amount` tokens to the seller.
    pub fn release_partial(env: Env, amount: i128) -> Result<(), EscrowError> {
        let buyer: Address = env
            .storage()
            .instance()
            .get(&DataKey::Buyer)
            .ok_or(EscrowError::NotInitialized)?;
        buyer.require_auth();

        let state: EscrowState = env
            .storage()
            .instance()
            .get(&DataKey::State)
            .ok_or(EscrowError::NotInitialized)?;
        if !matches!(state, EscrowState::Funded | EscrowState::Delivered) {
            return Err(EscrowError::InvalidState);
        }

        let stored_amount: i128 = env.storage().instance().get(&DataKey::Amount).unwrap();
        if amount > stored_amount {
            return Err(EscrowError::InsufficientFunds);
        }

        let seller: Address = env.storage().instance().get(&DataKey::Seller).unwrap();
        let token_contract: Address = env.storage().instance().get(&DataKey::TokenContract).unwrap();

        token::Client::new(&env, &token_contract)
            .transfer(&env.current_contract_address(), &seller, &amount);

        env.storage().instance().set(&DataKey::Amount, &(stored_amount - amount));
        bump_instance(&env);

        env.events()
            .publish((Symbol::new(&env, "partial_released"), seller), amount);
        Ok(())
    }

    /// Buyer cancels an unfunded escrow (`Created` state only).
    pub fn cancel(env: Env) -> Result<(), EscrowError> {
        let buyer: Address = env
            .storage()
            .instance()
            .get(&DataKey::Buyer)
            .ok_or(EscrowError::NotInitialized)?;
        buyer.require_auth();

        let state: EscrowState = env
            .storage()
            .instance()
            .get(&DataKey::State)
            .ok_or(EscrowError::NotInitialized)?;
        if state != EscrowState::Created {
            return Err(EscrowError::InvalidState);
        }

        env.storage().instance().set(&DataKey::State, &EscrowState::Cancelled);
        bump_instance(&env);
        Ok(())
    }

    /// Extend storage TTL. Anyone can call this to keep an active escrow alive.
    pub fn bump(env: Env) -> Result<(), EscrowError> {
        if !env.storage().instance().has(&DataKey::State) {
            return Err(EscrowError::NotInitialized);
        }
        bump_instance(&env);
        Ok(())
    }

    /// Pause the contract. Admin only.
    pub fn pause(env: Env) -> Result<(), EscrowError> {
        let admin = require_admin(&env)?;
        admin.require_auth();
        env.storage().instance().set(&DataKey::Paused, &true);
        bump_instance(&env);
        Ok(())
    }

    /// Upgrade the contract WASM. Admin only.
    pub fn upgrade(env: Env, new_wasm_hash: soroban_sdk::BytesN<32>) -> Result<(), EscrowError> {
        let admin = require_admin(&env)?;
        admin.require_auth();
        env.deployer().update_current_contract_wasm(new_wasm_hash);
        Ok(())
    }

    /// Return full escrow details as an [`EscrowInfo`] struct.
    pub fn get_escrow_info(env: Env) -> EscrowInfo {
        EscrowInfo {
            buyer: env.storage().instance().get(&DataKey::Buyer).unwrap(),
            seller: env.storage().instance().get(&DataKey::Seller).unwrap(),
            arbiter: env.storage().instance().get(&DataKey::Arbiter).unwrap(),
            token_contract: env.storage().instance().get(&DataKey::TokenContract).unwrap(),
            amount: env.storage().instance().get(&DataKey::Amount).unwrap(),
            deadline: env.storage().instance().get(&DataKey::Deadline).unwrap(),
            state: env.storage().instance().get(&DataKey::State).unwrap(),
        }
    }

    /// Return the current [`EscrowState`], or `None` if not initialized.
    pub fn get_state(env: Env) -> Option<EscrowState> {
        env.storage().instance().get(&DataKey::State)
    }

    /// Return `true` if the deadline ledger has been passed.
    pub fn is_deadline_passed(env: Env) -> bool {
        let deadline: u32 = env
            .storage()
            .instance()
            .get(&DataKey::Deadline)
            .unwrap_or(0);
        env.ledger().sequence() > deadline
    }
}

impl EscrowContract {
    fn release_to_seller(env: Env) -> Result<(), EscrowError> {
        let seller: Address = env.storage().instance().get(&DataKey::Seller).unwrap();
        let token_contract: Address = env.storage().instance().get(&DataKey::TokenContract).unwrap();
        let amount: i128 = env.storage().instance().get(&DataKey::Amount).unwrap();

        token::Client::new(&env, &token_contract)
            .transfer(&env.current_contract_address(), &seller, &amount);

        env.storage().instance().set(&DataKey::State, &EscrowState::Completed);
        bump_instance(&env);

        env.events()
            .publish((Symbol::new(&env, "funds_released"), seller), amount);
        Ok(())
    }

    fn refund_to_buyer(env: Env) -> Result<(), EscrowError> {
        let buyer: Address = env.storage().instance().get(&DataKey::Buyer).unwrap();
        let token_contract: Address = env.storage().instance().get(&DataKey::TokenContract).unwrap();
        let amount: i128 = env.storage().instance().get(&DataKey::Amount).unwrap();

        token::Client::new(&env, &token_contract)
            .transfer(&env.current_contract_address(), &buyer, &amount);

        env.storage().instance().set(&DataKey::State, &EscrowState::Refunded);
        bump_instance(&env);

        env.events()
            .publish((Symbol::new(&env, "funds_refunded"), buyer), amount);
        Ok(())
    }
}

#[cfg(test)]
mod test;
