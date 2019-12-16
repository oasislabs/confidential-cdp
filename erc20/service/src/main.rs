#[macro_use]
extern crate serde;

use map_vec::{map::Entry, Map, Set};
use oasis_std::{Address, Context, Event};

//pub type Result<T> = std::result::Result<T, String>;
pub type Result<T> = std::result::Result<T, Error>;

#[derive(Debug, PartialEq, Serialize, Deserialize, failure::Fail)]
pub enum Error {
    #[fail(display = "Unknown error occured.")]
    Unknown,

    #[fail(display = "Only existing admins can perform this operation.")]
    AdminPrivilegesRequired,

    #[fail(display = "Insuffient funds for transfer from {:?}.", address)]
    InsufficientFunds { address: Address },

    #[fail(display = "Address {:?} has no allowance from address {:?}.", from, to)]
    NoAllowanceGiven { from: Address, to: Address },

    #[fail(
        display = "Transfer request {} exceeds allowance {}.",
        amount, allowance
    )]
    RequestExceedsAllowance { amount: f64, allowance: f64 },
}

#[derive(oasis_std::Service, Default, Debug)]
pub struct ERC20Token {
    total_supply: f64,
    owner: Address,
    admins: Set<Address>,
    accounts: Map<Address, f64>,
    allowed: Map<Address, Map<Address, f64>>,
}

// A Transfer event struct
#[derive(Serialize, Deserialize, Clone, Debug, Default, Event)]
pub struct Transfer {
    pub from: Address,
    pub to: Address,
    pub amount: f64,
}

// An Approval event struct
#[derive(Serialize, Deserialize, Clone, Debug, Default, Event)]
pub struct Approval {
    pub sender: Address,
    pub spender: Address,
    pub amount: f64,
}

impl ERC20Token {
    /// Constructs a new `ERC20Token`
    pub fn new(ctx: &Context, total_supply: f64) -> Result<Self> {
        let owner = ctx.sender();
        let mut admins = Set::new();
        admins.insert(owner);
        let mut accounts = Map::new();
        accounts.insert(owner, total_supply);

        Ok(Self {
            total_supply,
            owner,
            admins,
            accounts,
            ..Default::default()
        })
    }

    // for debugging purposes
    pub fn show_all(&self, _ctx: &Context) -> String {
        format!("{:?}\n", self)
    }
    pub fn list_admin(&self, ctx: &Context) -> String {
        format!("You: {:?}\nlist: {:?}\n", ctx.sender(), self.admins)
    }
    pub fn erc20_addr(&self, ctx: &Context) -> String {
        format!("{:?}", ctx.address())
    }

    /// Get balance
    pub fn balance_of(&mut self, ctx: &Context) -> Result<f64> {
        eprintln!("erc20 balance_of called");
        Ok(self
            .accounts
            .get(&ctx.sender())
            .copied()
            .unwrap_or_default())
    }

    /// Get balance of contract
    pub fn balance_of_contract(&self, _ctx: &Context, addr: Address) -> Result<f64> {
        eprintln!("erc20 balance_of_contract called");
        Ok(self.accounts.get(&addr).copied().unwrap_or_default())
    }

    /// Get total supply
    pub fn total_supply(&mut self, _ctx: &Context) -> Result<f64> {
        Ok(self.total_supply)
    }

    /// Add admin
    pub fn add_admin(&mut self, ctx: &Context, admin: Address) -> Result<()> {
        if !self.admins.contains(&ctx.sender()) {
            return Err(Error::AdminPrivilegesRequired);
        }
        self.admins.insert(admin);
        Ok(())
    }
}

// Helper methods

/// transfer method
fn do_transfer(accounts: &mut Map<Address, f64>, from: Address, to: Address, amount: f64) -> bool {
    eprintln!("erc20 do_transfer called");
    let from_balance = accounts.get(&from).copied().unwrap_or_default();
    let to_balance = accounts.get(&to).copied().unwrap_or_default();
    eprintln!("balance of fromAddr: {}", from_balance);
    eprintln!("balance of toAddr: {}", to_balance);

    // check for sufficient balance
    if from_balance < amount {
        return false;
    }
    accounts.insert(from, from_balance - amount);
    accounts.insert(to, to_balance + amount);

    eprintln!("erc20 transfer - books updated");
    Event::emit(&Transfer { from, to, amount });

    true
}

impl ERC20Token {
    /// transfer
    pub fn transfer(&mut self, ctx: &Context, to: Address, amount: f64) -> Result<Transfer> {
        eprintln!("erc20: transfer called");
        let from = ctx.sender();
        if from == to || amount == 0f64 {
            // no-op
            return Ok(Transfer::default());
        }
        if do_transfer(&mut self.accounts, ctx.sender(), to, amount) {
            eprintln!("erc20 transfer success");
            return Ok(Transfer { from, to, amount });
        }
        Err(Error::InsufficientFunds { address: from })
    }

    /// transfer from contract
    pub fn transfer_to_from(&mut self, _ctx: &Context, 
        from: Address, to: Address, amount: f64) -> Result<Transfer> {
        eprintln!("erc20: transfer to/from called");
        if do_transfer(&mut self.accounts, from, to, amount) {
            return Ok(Transfer { from, to, amount });
        }
        Err(Error::InsufficientFunds { address: from })
    }

    // for debugging only
    /// getting tokens for testing purposes
    pub fn faucet(&mut self, ctx: &Context, amount: f64) -> Result<Transfer> {
        let to = ctx.sender();
        let mut admin = Address::default();
        for a in self.admins.iter() {
            admin = *a;
        }
        if do_transfer(&mut self.accounts, admin, to, amount) {
            return Ok(Transfer {
                from: admin,
                to,
                amount,
            });
        }
        self.total_supply += amount;
        Err(Error::InsufficientFunds { address: admin })
    }
    pub fn faucet_to_addr(
        &mut self,
        _ctx: &Context,
        addr: Address,
        amount: f64,
    ) -> Result<Transfer> {
        let to = addr;
        let mut admin = Address::default();
        for a in self.admins.iter() {
            admin = *a;
        }
        if do_transfer(&mut self.accounts, admin, to, amount) {
            return Ok(Transfer {
                from: admin,
                to,
                amount,
            });
        }
        self.total_supply += amount;
        Err(Error::InsufficientFunds { address: admin })
    }

    /// allowance
    pub fn approve(&mut self, ctx: &Context, spender: Address, amount: f64) -> Result<Approval> {
        let allowances = match self.allowed.entry(ctx.sender()) {
            Entry::Vacant(ve) => ve.insert(Map::new()),
            Entry::Occupied(oe) => oe.into_mut(),
        };
        allowances.insert(spender, amount);

        let approval = Approval {
            sender: ctx.sender(),
            spender,
            amount,
        };

        Event::emit(&approval);

        Ok(approval)
    }

    /// read allowance
    pub fn allowance(&mut self, ctx: &Context, spender: Address) -> Result<f64> {
        if !self.allowed.contains_key(&ctx.sender()) {
            return Ok(0f64);
        }
        Ok(self
            .allowed
            .get(&ctx.sender())
            .and_then(|allowances| allowances.get(&spender))
            .copied()
            .unwrap_or_default())
    }

    /// transfer from a given account up to the given allowance
    pub fn transfer_from(
        &mut self,
        _ctx: &Context,
        from: Address,
        spender: Address,
        amount: f64,
    ) -> Result<Transfer> {
        let allowances = self.allowed.get_mut(&from).unwrap();
        // if the spender is not in the list of addresses that are approved for automatic
        // withdrawal by the from address, then nothing can be done
        if !allowances.contains_key(&spender) {
            return Err(Error::NoAllowanceGiven { from, to: spender });
        }
        let allowance = allowances.get(&spender).copied().unwrap_or_default();
        // err if request is higher than allowance
        if allowance < amount {
            return Err(Error::RequestExceedsAllowance { amount, allowance });
        }
        if do_transfer(&mut self.accounts, from, spender, amount) {
            allowances.insert(spender, allowance - amount);
            return Ok(Transfer {
                from,
                to: spender,
                amount,
            });
        }
        Err(Error::InsufficientFunds { address: from })
    }
}

impl ERC20Token {
    /// mint new tokens
    pub fn mint(&mut self, ctx: &Context, amount: f64) -> Result<()> {
        if !self.admins.contains(&ctx.sender()) {
            return Err(Error::AdminPrivilegesRequired);
        }
        self.total_supply += amount;
        Ok(())
    }

    /// burn tokens from a given account
    pub fn burn(&mut self, ctx: &Context, from: Address, amount: f64) -> Result<()> {
        if !self.admins.contains(&ctx.sender()) {
            return Err(Error::AdminPrivilegesRequired);
        }
        let balance = self.accounts.get(&from).copied().unwrap_or_default();
        let mut new_amount = 0.0;
        if balance - amount > 0.0 {
            new_amount = balance - amount;
        }
        self.accounts.insert(from, new_amount);
        Ok(())
    }
}

fn main() {
    oasis_std::service!(ERC20Token);
}

#[cfg(test)]
mod tests {
    extern crate oasis_test;

    use super::*;
    use oasis_std::{Address, Context};

    /// Creates a new account and a `Context` with the new account as the sender.
    fn create_account() -> (Address, Context) {
        let addr = oasis_test::create_account(0 /* initial balance */);
        let ctx = Context::default().with_sender(addr).with_gas(100_000);
        (addr, ctx)
    }

    #[test]
    fn happy_paths() {
        let (_getafix, gctx) = create_account();
        let (_fulliautomatix, _fctx) = create_account();
        let (caesar, cctx) = create_account();
        let (brutus, bctx) = create_account();

        let mut erc20 = ERC20Token::new(&gctx, 1000.0).unwrap();
        eprintln!("total supply: {}", erc20.total_supply);

        // Getafix transfers a sum to Caesar
        let mut transfer = erc20.transfer(&gctx, caesar, 500.0).unwrap();
        eprintln!("{:?}", transfer);

        let mut balance = erc20.balance_of(&cctx).unwrap();
        assert_eq!(balance, 500.0f64);

        // Unsuspecting Caesar gives an allowance to Brutus
        let approval = erc20.approve(&cctx, brutus, 400.0).unwrap();
        eprintln!("{:?}", approval);
        balance = erc20.balance_of(&bctx).unwrap();
        assert_eq!(balance, 0.0f64);

        // Brutus transfer some tokens from Caesar
        transfer = erc20.transfer_from(&bctx, caesar, brutus, 400.0).unwrap();
        eprintln!("{:?}", transfer);
        balance = erc20.balance_of(&bctx).unwrap();
        assert_eq!(balance, 400.0f64);
    }
}
