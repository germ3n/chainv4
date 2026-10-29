use {
    super::RegistryError,
    crate::db::{Column, DbError, State, address_key},
    crate::structs::account::Account,
    crate::structs::address::Address,
    crate::structs::u256::u256,
    r#macro::document,
};

#[document(
    summary = "An account from an entry of the Accounts column, which holds multisigs' policies and proposals too, under keys of their own: the account when the key is an address's, exactly.",
    category = "Registries",
    see_also = "Accounts",
)]
pub fn account_entry(key: &[u8], value: &[u8]) -> Option<(Address, Account)> {
    let address = wincode::deserialize::<Address>(key).ok().filter(|address| address_key(address) == key)?;

    wincode::deserialize::<Account>(value).ok().map(|account| (address, account))
}

#[document(
    summary = "Balances and nonces by address. An address that was never written to reads as the default account, and an account that returns to the default is deleted, so the stored state only holds addresses that matter.",
    category = "Registries",
    see_also = "Account, State",
    example = "let accounts = Accounts::new(&tx);\naccounts.transfer(&from, &to, amount)?;\naccounts.bump_nonce(&from)?;",
)]
pub struct Accounts<'a> {
    state: &'a State,
}

impl<'a> Accounts<'a> {
    #[document(
        parent = "Accounts",
        summary = "Reads and writes accounts in the given state layer.",
    )]
    pub fn new(state: &'a State) -> Accounts<'a> {
        Accounts { state }
    }

    #[document(
        parent = "Accounts",
        summary = "The account at an address.",
        returns = "The account, or the default when there is none.",
    )]
    pub fn get(&self, address: &Address) -> Result<Account, DbError> {
        let account = self
            .state
            .get_value::<Account>(Column::Accounts, &address_key(address))?;

        Ok(account.unwrap_or_default())
    }

    #[document(
        parent = "Accounts",
        summary = "Stores an account, or deletes it when it is back to the default.",
    )]
    pub fn set(&self, address: &Address, account: &Account) {
        let key = address_key(address);

        if *account == Account::default() {
            self.state.delete(Column::Accounts, key);

            return;
        }

        self.state.put_value(Column::Accounts, key, account);
    }

    #[document(
        parent = "Accounts",
        summary = "Adds to a balance.",
        returns = "The account after the credit.",
        panics = "BalanceOverflow when the balance would pass the largest u256.",
    )]
    pub fn credit(&self, address: &Address, amount: u256) -> Result<Account, RegistryError> {
        let mut account = self.get(address)?;
        account.balance = account
            .balance
            .checked_add(amount)
            .ok_or(RegistryError::BalanceOverflow)?;
        self.set(address, &account);

        Ok(account)
    }

    #[document(
        parent = "Accounts",
        summary = "Takes from a balance.",
        returns = "The account after the debit.",
        panics = "InsufficientBalance when the account holds less than amount; nothing changes.",
    )]
    pub fn debit(&self, address: &Address, amount: u256) -> Result<Account, RegistryError> {
        let mut account = self.get(address)?;
        account.balance = account
            .balance
            .checked_sub(amount)
            .ok_or(RegistryError::InsufficientBalance)?;
        self.set(address, &account);

        Ok(account)
    }

    #[document(
        parent = "Accounts",
        summary = "Moves an amount from one address to another. Sending to yourself leaves the balance as it was.",
        panics = "InsufficientBalance or BalanceOverflow, with nothing changed.",
    )]
    pub fn transfer(&self, from: &Address, to: &Address, amount: u256) -> Result<(), RegistryError> {
        // Check both sides before changing either, so a failure leaves both accounts untouched.
        if self.get(from)?.balance < amount {
            return Err(RegistryError::InsufficientBalance);
        }

        if from != to && self.get(to)?.balance.checked_add(amount).is_none() {
            return Err(RegistryError::BalanceOverflow);
        }

        self.debit(from, amount)?;
        self.credit(to, amount)?;

        Ok(())
    }

    #[document(
        parent = "Accounts",
        summary = "Uses up the account's next nonce.",
        returns = "The nonce the transaction must have carried: the value before the bump.",
    )]
    pub fn bump_nonce(&self, address: &Address) -> Result<u64, RegistryError> {
        let mut account = self.get(address)?;
        let nonce = account.nonce;
        account.nonce = nonce.checked_add(1).ok_or(RegistryError::NonceOverflow)?;
        self.set(address, &account);

        Ok(nonce)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::{Database, MemoryDb, Read};
    use std::sync::Arc;

    const ALICE: Address = Address::Ed25519([1; 32]);
    const BOB: Address = Address::Ed25519([2; 32]);

    fn state() -> State {
        State::new(Arc::new(MemoryDb::default()))
    }

    #[test]
    fn unknown_addresses_are_empty_accounts() {
        let state = state();

        assert_eq!(Accounts::new(&state).get(&ALICE).unwrap(), Account::default());
    }

    #[test]
    fn transfers_move_balance() {
        let state = state();
        let accounts = Accounts::new(&state);
        accounts.credit(&ALICE, u256::new(100)).unwrap();
        accounts.transfer(&ALICE, &BOB, u256::new(30)).unwrap();

        assert_eq!(accounts.get(&ALICE).unwrap().balance, u256::new(70));
        assert_eq!(accounts.get(&BOB).unwrap().balance, u256::new(30));

        accounts.transfer(&ALICE, &ALICE, u256::new(70)).unwrap();

        assert_eq!(accounts.get(&ALICE).unwrap().balance, u256::new(70));
    }

    #[test]
    fn failed_moves_change_nothing() {
        let state = state();
        let accounts = Accounts::new(&state);
        accounts.credit(&ALICE, u256::new(10)).unwrap();
        accounts.credit(&BOB, u256::MAX).unwrap();

        assert_eq!(
            accounts.transfer(&ALICE, &BOB, u256::new(11)),
            Err(RegistryError::InsufficientBalance)
        );
        assert_eq!(
            accounts.transfer(&ALICE, &BOB, u256::new(1)),
            Err(RegistryError::BalanceOverflow)
        );
        assert_eq!(accounts.get(&ALICE).unwrap().balance, u256::new(10));
        assert_eq!(accounts.get(&BOB).unwrap().balance, u256::MAX);
    }

    #[test]
    fn nonces_count_up() {
        let state = state();
        let accounts = Accounts::new(&state);

        assert_eq!(accounts.bump_nonce(&ALICE).unwrap(), 0);
        assert_eq!(accounts.bump_nonce(&ALICE).unwrap(), 1);
        assert_eq!(accounts.get(&ALICE).unwrap().nonce, 2);
    }

    #[test]
    fn empty_accounts_are_deleted_and_full_ones_committed() {
        let db = Arc::new(MemoryDb::default());
        let state = State::new(db.clone());
        let accounts = Accounts::new(&state);
        accounts.credit(&ALICE, u256::new(5)).unwrap();
        accounts.credit(&BOB, u256::new(5)).unwrap();
        accounts.debit(&BOB, u256::new(5)).unwrap();
        db.commit(state.take_changes()).unwrap();

        assert!(db.read(Column::Accounts, &address_key(&ALICE)).unwrap().is_some());
        assert!(db.read(Column::Accounts, &address_key(&BOB)).unwrap().is_none());
    }
}
