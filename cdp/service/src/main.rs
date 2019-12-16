#[macro_use]
extern crate serde;

use erc20::Erc20TokenClient;
use failure::Fail;
use map_vec::{Map, Set};
use oasis_std::{exe::RpcError, Address, Context, Service};
use serde_json::json;
use std::time::SystemTime;

type Result<T> = std::result::Result<T, Error>;

#[derive(Debug, Serialize, Deserialize, Fail)]
pub enum Error {
    #[fail(display = "Unknown error occurred.")]
    Unknown,
    #[fail(display = "Time calculation went wrong. Try again")]
    TimeError,
    #[fail(display = "No account found.")]
    NoAccount,
    #[fail(display = "Account is already opened.")]
    AccountAlreadyOpened,
    #[fail(display = "Insufficient funds for transfer from {:?}.", addr)]
    InsufficientFunds { addr: Address },
    #[fail(display = "Insufficient underlying asset. You have: {}.", underlying)]
    InsufficientUnderlying { underlying: f64 },
    #[fail(display = "Insufficient liquidity. Shortfall: {}.", shortfall)]
    InsufficientCollateral { shortfall: f64 },
    #[fail(display = "Money market has insufficient cash to lend out.")]
    InsufficientCash,
    #[fail(display = "Money market has insufficient supply of otokens.")]
    InsufficientSupply,
    #[fail(display = "Admin privilege needed.")]
    AdminPrivilegesRequired,
    #[fail(display = "Money market is already listed.")]
    MarketAlreadyListed,
    #[fail(display = "Money market is not listed.")]
    MarketNotListed,
    #[fail(display = "Erc20 Error: {:?}", erc20_error)]
    Erc20Error { erc20_error: erc20::Error },
}

impl From<erc20::Error> for Error {
    fn from(error: erc20::Error) -> Self {
        Error::Erc20Error { erc20_error: error }
    }
}

fn approx_eq(a: f64, b: f64) -> bool {
    use std::f64;

    let same_sign = a.is_sign_positive() == b.is_sign_positive();
    let equal = ((a - b).abs() / f64::min(a.abs() + b.abs(), f64::MAX)) < f64::EPSILON;
    (same_sign && equal)
}

#[derive(Debug, Copy, Clone, Serialize, Deserialize)]
struct Position {
    // NOTE prod should handle f64 overflow for overdeposit/borrow
    underlying_asset: f64,
    otokens: f64,
    borrowed_asset: f64,
    last_checkpoint: SystemTime,
}

impl Default for Position {
    fn default() -> Self {
        Self {
            last_checkpoint: SystemTime::UNIX_EPOCH,
            ..Default::default()
        }
    }
}

/// For displaying information in frontend
#[derive(Debug, Serialize, Deserialize)]
struct MMInfo {
    exchange_rate: f64,
    borrow_rate: f64,
    earn_rate: f64,
    utilization_rate: f64,
    price_to_usd: f64,
    collateral_factor: f64,
}

#[derive(Debug, Serialize, Deserialize)]
struct MoneyMarket {
    name: String,
    total_lent: f64,
    total_supply: f64,
    account_position: Map<Address, Position>,
    collateral_factor: f64,
    erc20_addr: Address,
    // NOTE prod should build actual price oracle
    // { ERC20 addr : price to USD }
    // ex) 1 ERC20A == $250 { ERC20A : 250 }
    //price_oracle: Map<Address, f64>,
    price_to_usd: f64,
    last_checkpoint: SystemTime,
}

impl MoneyMarket {
    const INIT_EX_RATE: f64 = 0.02;
    const BASE_BORROW_IR: f64 = 0.025;

    pub fn new(name: String, token_addr: Address, price: f64) -> Self {
        Self {
            name,
            total_lent: 0.0,
            total_supply: 0.0,
            account_position: Map::new(),
            collateral_factor: 0.75,
            erc20_addr: token_addr,
            price_to_usd: price,
            last_checkpoint: SystemTime::now(),
        }
    }

    /// Core functions
    /// Users cannot directly call these functions
    /// they must go thru the controller to mint/borrow/redeem
    fn _mint(&mut self, ctx: &Context, amount: f64) -> Result<()> {
        eprintln!("_mint called");
        self._accrue_interest()?;

        let minted_otokens = amount / self.get_exchange_rate(ctx);

        if let Some(pre_position) = self.account_position.get_mut(&ctx.sender()) {
            eprintln!("_mint for existing account");
            pre_position.otokens += minted_otokens;
            pre_position.underlying_asset += amount;
        } else {
            eprintln!("_mint for new account");
            self._open_account(ctx, amount, minted_otokens, 0.0)?;
        }

        eprintln!("mint: internal books updated. Erc20 transfer started");
        // NOTE: typically, sender needs to approve this contract first
        // currently delegatecalls are allowed, so bypass approve process
        let mut erc20 = Erc20TokenClient::at(self.erc20_addr);
        // NOTE: ERC20 decimal not implemented so f64 is used
        // NOTE: If erc20 RPC's fail, entire tx is reverted
        erc20
            .transfer_to_from(ctx, ctx.sender(), ctx.address(), amount)
            .map_err(|err| match err {
                RpcError::Exec(e) => e,
                _ => panic!("Something went wrong."),
            })?;
        eprintln!("mint: erc20 transfer done");

        self.total_supply += minted_otokens;
        Ok(())
    }

    // Assuming `amount` is in underlying asset unit
    fn _redeem(&mut self, ctx: &Context, amount: f64) -> Result<()> {
        eprintln!("_redeem called");
        self._accrue_interest()?;

        let otokens_to_burn = amount / self.get_exchange_rate(ctx);
        if self.total_supply < otokens_to_burn {
            return Err(Error::InsufficientSupply);
        }

        let total_cash = self.get_total_cash(ctx);
        if total_cash < amount {
            return Err(Error::InsufficientCash);
        }

        // redeem/borrow can't assume user has balance in this mm
        // case: sender has enough liquidity in MM1 and
        // want to borrow from MM2
        if let Some(pre_position) = self.account_position.get_mut(&ctx.sender()) {
            // need to check user's underlying balance > amount
            // in addition to having enough liquidity (already done)
            if pre_position.underlying_asset < amount {
                return Err(Error::InsufficientUnderlying {
                    underlying: pre_position.underlying_asset,
                });
            }
            pre_position.otokens -= otokens_to_burn;
            pre_position.underlying_asset -= amount;
        } else {
            return Err(Error::NoAccount);
        }

        let mut erc20 = Erc20TokenClient::at(self.erc20_addr);
        erc20
            .transfer_to_from(ctx, ctx.address(), ctx.sender(), amount)
            .map_err(|err| match err {
                RpcError::Exec(e) => e,
                _ => panic!("Something went wrong."),
            })?;

        self.total_supply -= otokens_to_burn;
        Ok(())
    }

    fn _borrow(&mut self, ctx: &Context, amount: f64) -> Result<()> {
        eprintln!("_borrow called");
        self._accrue_interest()?;

        let total_cash = self.get_total_cash(ctx);
        if total_cash < amount {
            return Err(Error::InsufficientCash);
        }

        if let Some(pre_position) = self.account_position.get_mut(&ctx.sender()) {
            pre_position.borrowed_asset += amount;
        } else {
            self._open_account(ctx, 0.0, 0.0, amount)?;
        }

        // transfer cash to borrower
        eprintln!("_borrow erc20 transfer starting");
        let mut erc20 = Erc20TokenClient::at(self.erc20_addr);
        erc20
            .transfer_to_from(ctx, ctx.address(), ctx.sender(), amount)
            .map_err(|err| match err {
                RpcError::Exec(e) => e,
                _ => panic!("Something went wrong."),
            })?;
        eprintln!("_borrow erc20 transfer done");

        self.total_lent += amount;
        Ok(())
    }

    pub fn _repay_borrow(&mut self, ctx: &Context, amount: f64) -> Result<()> {
        self._accrue_interest()?;

        if let Some(pre_position) = self.account_position.get_mut(&ctx.sender()) {
            pre_position.borrowed_asset -= amount;
        } else {
            return Err(Error::NoAccount);
        }

        let mut erc20 = Erc20TokenClient::at(self.erc20_addr);
        erc20
            .transfer_to_from(ctx, ctx.sender(), ctx.address(), amount)
            .map_err(|err| match err {
                RpcError::Exec(e) => e,
                _ => panic!("Something went wrong."),
            })?;

        self.total_lent -= amount;
        Ok(())
    }

    // TODO
    pub fn liquidate(&mut self, _ctx: &Context) -> Result<()> {
        Ok(())
    }

    fn _open_account(
        &mut self,
        ctx: &Context,
        underlying_amt: f64,
        otoks: f64,
        borrowed_amt: f64,
    ) -> Result<()> {
        eprintln!("_open_account called");
        if let Some(_) = self.account_position.get(&ctx.sender()) {
            return Err(Error::AccountAlreadyOpened);
        }

        self.account_position.insert(
            ctx.sender(),
            Position {
                underlying_asset: underlying_amt,
                otokens: otoks,
                borrowed_asset: borrowed_amt,
                last_checkpoint: SystemTime::now(),
            },
        );
        eprintln!("account opened");
        Ok(())
    }

    /// Market Info Getters
    pub fn get_total_cash(&self, ctx: &Context) -> f64 {
        eprintln!("get total cash called");
        let erc20 = Erc20TokenClient::at(self.erc20_addr);
        let cash = erc20.balance_of_contract(ctx, ctx.address()).unwrap_or(0.0);
        eprintln!("cash is {}", cash);
        cash
    }

    // exchange increases as market borrow balance grows from
    // interest accrued by borrowers (not guaranteed to grow)
    pub fn get_exchange_rate(&self, ctx: &Context) -> f64 {
        eprintln!("get exchange rate called");
        let total_cash = self.get_total_cash(ctx);
        eprintln!("total cash is {}", total_cash);
        if self.total_supply == 0.0 || (total_cash == 0.0 && self.total_lent == 0.0) {
            eprintln!("return initial ex_rate");
            return Self::INIT_EX_RATE;
        }

        (total_cash + self.total_lent) / self.total_supply
    }

    pub fn get_borrow_rate(&self, ctx: &Context) -> f64 {
        let bir = Self::BASE_BORROW_IR + 0.2 * self.get_utilization_ratio(ctx);
        eprintln!("borrow rate: {}", bir);
        bir
    }

    // no earn IR if no borrows happening
    pub fn get_earn_rate(&self, ctx: &Context) -> f64 {
        eprintln!("get earn rate called");
        self.get_borrow_rate(ctx) * self.get_utilization_ratio(ctx)
    }

    pub fn get_rates(&self, ctx: &Context) -> (f64, f64, f64, f64) {
        eprintln!("getting all rates");
        (
            self.get_exchange_rate(ctx),
            self.get_borrow_rate(ctx),
            self.get_earn_rate(ctx),
            self.get_utilization_ratio(ctx),
        )
    }

    pub fn get_utilization_ratio(&self, ctx: &Context) -> f64 {
        eprintln!("get utilization ratio called");
        let total_lent = self.total_lent;
        let total_cash = self.get_total_cash(ctx);

        if total_lent + total_cash <= 0.0 {
            eprintln!("util ratio: total lent + cash == 0");
            return 0.0;
        }
        let util_ratio = total_lent / (total_lent + total_cash);
        eprintln!("util ratio: {}", util_ratio);
        util_ratio
    }

    fn _accrue_interest(&mut self) -> Result<()> {
        eprintln!("accrue interest called");
        let now = SystemTime::now();
        let dur = match now.duration_since(self.last_checkpoint) {
            Ok(d) => d.as_secs_f64(),
            Err(_) => return Err(Error::TimeError),
        };

        let dur_yr = dur / 3600.0 / 24.0 / 364.25; // in years
        eprintln!("duration since last check point {} years", dur_yr);

        // interest factor = r * t
        let interest_factor = self.get_borrow_rate(&Context::default()) * dur_yr;
        let interest = self.total_lent * interest_factor;
        eprintln!("interest to accumulate {}", interest);

        if approx_eq(interest, 0.0) {
            eprintln!("time too short for interest to accumulate");
            return Ok(());
        }
        let interest_accumulated = interest;

        self.total_lent += interest_accumulated;
        self.last_checkpoint = now;
        eprintln!(
            "interest factor {} - accrued {}",
            interest_factor, interest_accumulated
        );
        Ok(())
    }
}

#[derive(Service, Debug, Serialize, Deserialize)]
struct Cdp {
    admins: Set<Address>,
    mm_map: Map<String, MoneyMarket>,
}

impl Cdp {
    pub fn new(ctx: &Context) -> Self {
        let mut a = Set::new();
        a.insert(ctx.sender());
        Self {
            admins: a,
            mm_map: Map::new(),
        }
    }

    // for debugging purposes
    pub fn show_all(&self, _ctx: &Context) -> String {
        let j = serde_json::to_string(&self).unwrap_or_else(|_| {
            return format!("NOT JSON {:?}\n", self);
        });
        format!("JSON {}\n", j)
    }

    /// CDP Info Getters
    pub fn list_admin(&self, ctx: &Context) -> String {
        format!("You: {:?}\nlist: {:?}\n", ctx.sender(), self.admins)
    }
    pub fn cdp_addr(&self, ctx: &Context) -> String {
        format!("{:?}", ctx.address())
    }

    /// Market Info Getters
    pub fn get_admin_market(&self, ctx: &Context, mm_name: &str) -> String {
        if !self.admins.contains(&ctx.sender()) {
            return format!("Admin privileges required");
        }
        if !self.mm_listed(ctx, mm_name) {
            eprintln!("MM not listed");
            return format!("MM not listed");
        }

        let market = self.mm_map.get(mm_name).unwrap();
        let j = serde_json::to_string(market).unwrap_or_else(|_| {
            return format!("NOT JSON {:?}\n", market);
        });
        format!("JSON {}\n", j)
    }
    pub fn get_market_info(&self, ctx: &Context, mm_name: &str) -> String {
        if !self.mm_listed(ctx, mm_name) {
            eprintln!("MM not listed");
            return format!("MM not listed");
        }
        let market = self.mm_map.get(mm_name).unwrap();
        let cash = self.mm_map.get(mm_name).unwrap().get_total_cash(ctx);

        let (exr, br, er, ur) = self.mm_map.get(mm_name).unwrap().get_rates(ctx);
        let j = json!({
            "Collateral Factor" : market.collateral_factor,
            "Price in USD": market.price_to_usd,
            "Market Liquidity": cash,
            "Exchange Rate": exr,
            "Borrow APR": br,
            "Earn APR": er,
            "Utilization Ratio": ur,
        });
        format!("{}", j)
    }

    /// User Info Getters
    pub fn get_user_global_position(&self, ctx: &Context) -> String {
        let liquidity = self.get_hypo_acct_liquidity(ctx, 0.0, "");
        let (sum_collateral, sum_borrow) = self.get_sum_collat_borrow(ctx);
        let j = json!({
            "Current Liquidity": liquidity,
            "Total Collateral": sum_collateral,
            "Total Borrowed": sum_borrow
        });

        format!("{}", j)
    }
    pub fn get_user_mm_position(&self, ctx: &Context, mm_name: &str) -> String {
        if !self.mm_listed(ctx, mm_name) {
            eprintln!("MM not listed");
            return format!("MM not listed");
        }

        let position = self
            .mm_map
            .get(mm_name)
            .unwrap()
            .account_position
            .get(&ctx.sender())
            .copied()
            .unwrap_or_default();

        let j = serde_json::to_string(&position).unwrap_or_else(|_| {
            return format!("NOT JSON {:?}\n", position);
        });
        format!("JSON {}\n", j)
    }

    /// Core CDP functions
    pub fn add_market(
        &mut self,
        ctx: &Context,
        name: &str,
        price_to_usd: f64,
        erc20_addr: Address,
    ) -> Result<()> {
        eprintln!("add market called");
        if !self.admins.contains(&ctx.sender()) {
            eprintln!("Not admin error");
            return Err(Error::AdminPrivilegesRequired);
        }
        if self.mm_listed(ctx, name) {
            eprintln!("MM already listed");
            return Err(Error::MarketAlreadyListed);
        }
        eprintln!("MM being added");
        let new_mm = MoneyMarket::new(name.to_string(), erc20_addr, price_to_usd);
        self.mm_map.insert(name.to_string(), new_mm);
        eprintln!("MM added");
        Ok(())
    }

    pub fn mint(&mut self, ctx: &Context, mint_amount: f64, mm_name: &str) -> Result<()> {
        eprintln!("mint called");
        if !self.mm_listed(ctx, mm_name) {
            eprintln!("market not listed");
            return Err(Error::MarketNotListed);
        }
        eprintln!("minting");
        let market = self.mm_map.get_mut(mm_name).unwrap();
        market._mint(ctx, mint_amount)?;
        eprintln!("minting done");
        Ok(())
    }

    pub fn borrow(&mut self, ctx: &Context, borrow_amount: f64, mm_name: &str) -> Result<()> {
        if !self.mm_listed(ctx, mm_name) {
            return Err(Error::MarketNotListed);
        }
        let hypo_liquidity = self.get_hypo_acct_liquidity(ctx, borrow_amount, mm_name);
        if hypo_liquidity < 0.0 {
            return Err(Error::InsufficientCollateral {
                shortfall: hypo_liquidity,
            });
        }
        let market = self.mm_map.get_mut(mm_name).unwrap();
        market._borrow(ctx, borrow_amount)?;
        Ok(())
    }

    pub fn repay_borrow(&mut self, ctx: &Context, repay_amount: f64, mm_name: &str) -> Result<()> {
        if !self.mm_listed(ctx, mm_name) {
            return Err(Error::MarketNotListed);
        }

        let market = self.mm_map.get_mut(mm_name).unwrap();
        market._repay_borrow(ctx, repay_amount)?;
        Ok(())
    }

    pub fn redeem(&mut self, ctx: &Context, redeem_amount: f64, mm_name: &str) -> Result<()> {
        if !self.mm_listed(ctx, mm_name) {
            return Err(Error::MarketNotListed);
        }
        let hypo_liquidity = self.get_hypo_acct_liquidity(ctx, redeem_amount, mm_name);
        if hypo_liquidity < 0.0 {
            return Err(Error::InsufficientCollateral {
                shortfall: hypo_liquidity,
            });
        }
        let market = self.mm_map.get_mut(mm_name).unwrap();
        market._redeem(ctx, redeem_amount)?;
        Ok(())
    }

    // returns hypothetical liquidity after amount taken out
    // pass in non-existing mm_name to get current liquidity
    pub fn get_hypo_acct_liquidity(
        &self,
        ctx: &Context,
        takeout_amount: f64,
        mm_name: &str,
    ) -> f64 {
        eprintln!("getting hypothetical account liquidity");
        let (sum_collateral, mut sum_borrow_plus_effect) = self.get_sum_collat_borrow(ctx);

        if let Some(mm) = self.mm_map.get(mm_name) {
            let takeout_effect = mm.price_to_usd * takeout_amount;
            eprintln!("effect of taking out money is: {}", takeout_effect);
            sum_borrow_plus_effect += takeout_effect;
        }

        let hypo_liquidity = sum_collateral - sum_borrow_plus_effect;
        eprintln!("hypo acct liq: {}", hypo_liquidity);
        hypo_liquidity
    }

    pub fn get_sum_collat_borrow(&self, ctx: &Context) -> (f64, f64) {
        let (mut sum_collateral, mut sum_borrow) = (0.0f64, 0.0f64);
        for (market_name, market) in self.mm_map.iter() {
            eprintln!("inspecting acct position in {}", market_name);
            if let Some(position) = market.account_position.get(&ctx.sender()) {
                let otoken_balance = position.otokens;
                let borrow_balance = position.borrowed_asset;
                let exchange_rate = market.get_exchange_rate(ctx);
                let collateral_factor = market.collateral_factor;
                let oracle_price = market.price_to_usd;

                let collateral = collateral_factor * exchange_rate * oracle_price * otoken_balance;
                let borrowed = oracle_price * borrow_balance;
                eprintln!(
                    "account has {} collat, {} borrow balance",
                    collateral, borrowed
                );
                sum_collateral += collateral;
                sum_borrow += borrowed;
            } // if user has no position in this mm, skip to next
        }
        (sum_collateral, sum_borrow)
    }

    pub fn mm_listed(&self, _ctx: &Context, mm_name: &str) -> bool {
        if self.mm_map.contains_key(mm_name) {
            return true;
        }
        false
    }

    pub fn change_price_oracle(&mut self, ctx: &Context, mm_name: &str, price: f64) -> Result<()> {
        eprintln!("change price oracle called");
        if !self.admins.contains(&ctx.sender()) {
            eprintln!("Not admin error");
            return Err(Error::AdminPrivilegesRequired);
        }
        if !self.mm_listed(ctx, mm_name) {
            eprintln!("MM not listed");
            return Err(Error::MarketNotListed);
        }
        let market = self.mm_map.get_mut(mm_name).unwrap();
        market.price_to_usd = price;
        eprintln!("price changed");
        Ok(())
    }

    pub fn change_collateral_factor(
        &mut self,
        ctx: &Context,
        mm_name: &str,
        factor: f64,
    ) -> Result<()> {
        if !self.admins.contains(&ctx.sender()) {
            return Err(Error::AdminPrivilegesRequired);
        }
        if !self.mm_listed(ctx, mm_name) {
            return Err(Error::MarketNotListed);
        }
        let market = self.mm_map.get_mut(mm_name).unwrap();
        market.collateral_factor = factor;
        Ok(())
    }
}

fn main() {
    oasis_std::service!(Cdp);
}

#[cfg(test)]
mod tests {
    extern crate oasis_test;

    use super::*;

    fn create_account_ctx() -> (Address, Context) {
        let addr = oasis_test::create_account(100);
        let ctx = Context::default().with_sender(addr).with_gas(100_000);
        (addr, ctx)
    }

    #[test]
    fn test() {
        let (_admin, mut admin_ctx) = create_account_ctx();
        let (_lender, mut lender_ctx) = create_account_ctx();
        let (_borrower, mut borrower_ctx) = create_account_ctx();

        let sender = oasis_test::create_account(1);
        let ctx = Context::default().with_sender(sender);
        let mut cdp = Cdp::new(&ctx);
        eprintln!("{:?}", cdp);

        cdp.add_market(&ctx, "oERC20A".to_string(), 250.0, Address::default());
        eprintln!("{:?}", cdp);
    }
}
