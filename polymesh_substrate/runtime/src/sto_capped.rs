use crate::asset;
use crate::asset::AssetTrait;
use crate::erc20;
use crate::erc20::ERC20Trait;
use crate::identity;

use crate::utils;
use support::traits::Currency;

use rstd::prelude::*;
use runtime_primitives::traits::{As, CheckedAdd, CheckedMul};
use support::{decl_event, decl_module, decl_storage, dispatch::Result, ensure, StorageMap};
use system::{self, ensure_signed};

/// The module's configuration trait.
pub trait Trait: timestamp::Trait + system::Trait + utils::Trait + balances::Trait {
    // TODO: Add other types and constants required configure this module.

    /// The overarching event type.
    type Event: From<Event<Self>> + Into<<Self as system::Trait>::Event>;
    type Asset: asset::AssetTrait<Self::AccountId, Self::TokenBalance>;
    type Identity: identity::IdentityTrait<Self::AccountId>;
    type ERC20Trait: erc20::ERC20Trait<Self::AccountId, Self::TokenBalance>;
}

#[derive(parity_codec::Encode, parity_codec::Decode, Default, Clone, PartialEq, Debug)]
pub struct STO<U, V, W> {
    beneficiary: U,
    cap: V,
    sold: V,
    rate: u64,
    start_date: W,
    end_date: W,
    active: bool,
}

#[derive(parity_codec::Encode, parity_codec::Decode, Default, Clone, PartialEq, Debug)]
pub struct Investment<U, V, W> {
    investor: U,
    amount_payed: V,
    tokens_purchased: V,
    purchase_date: W,
}

decl_storage! {
    trait Store for Module<T: Trait> as STOCapped {

        // Tokens can have multiple whitelists that (for now) check entries individually within each other
        StosByToken get(stos_by_token): map (Vec<u8>, u32) => STO<T::AccountId,T::TokenBalance,T::Moment>;

        StoCount get(sto_count): map (Vec<u8>) => u32;

        // List of ERC20 tokens which will be accepted as the investment currency for the STO
        // [asset_ticker][sto_id][index] => erc20_ticker
        AllowedTokens get(allowed_tokens): map(Vec<u8>, u32, u32) => Vec<u8>;
        // To track the index of the token address for the given STO
        // [Asset_ticker][sto_id][erc20_ticker] => index
        TokenIndexForSTO get(token_index_for_sto): map(Vec<u8>, u32, Vec<u8>) => Option<u32>;
        // To track the no of different tokens allowed as investment currency for the given STO
        // [asset_ticker][sto_id] => count
        TokensCountForSto get(tokens_count_for_sto): map(Vec<u8>, u32) => u32;
    }
}

decl_module! {
    /// The module declaration.
    pub struct Module<T: Trait> for enum Call where origin: T::Origin {
        // Initializing events
        // this is needed only if you are using events in your module
        fn deposit_event<T>() = default;

        pub fn launch_sto(origin, _ticker: Vec<u8>, beneficiary: T::AccountId, cap: T::TokenBalance, rate: u64, start_date: T::Moment, end_date: T::Moment) -> Result {
            let sender = ensure_signed(origin)?;
            let ticker = utils::bytes_to_upper(_ticker.as_slice());
            ensure!(Self::is_owner(ticker.clone(),sender.clone()),"Sender must be the token owner");

            let sto = STO {
                beneficiary,
                cap,
                sold:<T::TokenBalance as As<u64>>::sa(0),
                rate,
                start_date,
                end_date,
                active: true
            };

            let sto_count = Self::sto_count(ticker.clone());
            let new_sto_count = sto_count
                .checked_add(1)
                .ok_or("overflow in calculating next sto count")?;

            <StosByToken<T>>::insert((ticker.clone(),sto_count), sto);
            <StoCount<T>>::insert(ticker.clone(),new_sto_count);

            runtime_io::print("Capped STOlaunched!!!");

            Ok(())
        }

        pub fn buy_tokens(origin, _ticker: Vec<u8>, sto_id: u32, value: T::Balance ) -> Result {
            let sender = ensure_signed(origin)?;
            let ticker = utils::bytes_to_upper(_ticker.as_slice());

            //PABLO: TODO: Validate that buyer is whitelisted for primary issuance.

            let mut selected_sto = Self::stos_by_token((ticker.clone(),sto_id));

            let now = <timestamp::Module<T>>::get();
            ensure!(now >= selected_sto.start_date && now <= selected_sto.end_date,"STO has not started or already ended");

            // Make sure sender has enough balance
            let sender_balance = <balances::Module<T> as Currency<_>>::free_balance(&sender);

            ensure!(sender_balance >= value,"Insufficient funds");

            //  Calculate tokens to mint
            let token_conversion = <T::TokenBalance as As<T::Balance>>::sa(value).checked_mul(&<T::TokenBalance as As<u64>>::sa(selected_sto.rate))
                .ok_or("overflow in calculating tokens")?;

            selected_sto.sold = selected_sto.sold
                .checked_add(&token_conversion)
                .ok_or("overflow while calculating tokens sold")?;

            // Make sure there's still an allocation
            // PABLO: TODO: Instead of reverting, buy up to the max and refund excess of poly.
            ensure!(selected_sto.sold <= selected_sto.cap, "There's not enough tokens");

            // Mint tokens and update STO
            T::Asset::_mint_from_sto(ticker.clone(), sender.clone(), token_conversion)?;

            // Transfer poly to token owner
            <balances::Module<T> as Currency<_>>::transfer(
                &sender,
                &selected_sto.beneficiary,
                value
                )?;

            <StosByToken<T>>::insert((ticker.clone(),sto_id), selected_sto);
            // PABLO: TODO: Store Investment DATA

            // PABLO: TODO: Emit event

            runtime_io::print("Invested in STO");

            Ok(())
        }

        pub fn modify_allowed_tokens(origin, _ticker: Vec<u8>, sto_id: u32, erc20_ticker: Vec<u8>, modify_status: bool) -> Result {
            let sender = ensure_signed(origin)?;
            let ticker = utils::bytes_to_upper(_ticker.as_slice());

            let selected_sto = Self::stos_by_token((ticker.clone(),sto_id));
            let now = <timestamp::Module<T>>::get();
            // Right now we are only allowing the issuer to change the configuration only before the STO start not after the start
            // or STO should be in non-active stage
            ensure!(now < selected_sto.start_date || !selected_sto.active, "STO is already started");

            ensure!(Self::is_owner(ticker.clone(),sender), "Not authorised to execute this function");

            let token_index = Self::token_index_for_sto((ticker.clone(), sto_id, erc20_ticker.clone()));
            let token_count = Self::tokens_count_for_sto((ticker.clone(), sto_id));

            let current_status = match token_index == None {
                true => false,
                false => true,
            };

            ensure!(current_status != modify_status, "Already in that state");

            if modify_status {
                let new_count = token_count.checked_add(1).ok_or("overflow new token count value")?;
                <TokenIndexForSTO<T>>::insert((ticker.clone(), sto_id, erc20_ticker.clone()), new_count);
                <AllowedTokens<T>>::insert((ticker.clone(), sto_id, new_count), erc20_ticker.clone());
                <TokensCountForSto<T>>::insert((ticker.clone(), sto_id), new_count);
            } else {
                let new_count = token_count.checked_sub(1).ok_or("underflow new token count value")?;
                <TokenIndexForSTO<T>>::insert((ticker.clone(), sto_id, erc20_ticker.clone()), new_count);
                <AllowedTokens<T>>::insert((ticker.clone(), sto_id, new_count), vec![]);
                <TokensCountForSto<T>>::insert((ticker.clone(), sto_id), new_count);
            }

            Self::deposit_event(RawEvent::ModifyAllowedTokens(ticker, erc20_ticker, sto_id, modify_status));

            Ok(())

        }

        pub fn buy_tokens_by_erc20(origin, _ticker: Vec<u8>, sto_id: u32, value: T::TokenBalance, erc20_ticker: Vec<u8>) -> Result {
            let sender = ensure_signed(origin)?;
            let ticker = utils::bytes_to_upper(_ticker.as_slice());

            // Check whether given token is allowed as investment currency or not
            ensure!(Self::token_index_for_sto((ticker.clone(), sto_id, erc20_ticker.clone())) != None, "Given token is not a permitted investment currency");
            let mut selected_sto = Self::stos_by_token((ticker.clone(),sto_id));

            let now = <timestamp::Module<T>>::get();
            ensure!(now >= selected_sto.start_date && now <= selected_sto.end_date, "STO has not started or already ended");
            ensure!(selected_sto.active, "STO is not active at the moment");
            ensure!(T::ERC20Trait::balanceOf(erc20_ticker.clone(), sender.clone()) >= value, "Insufficient balance");

            //  Calculate tokens to mint
            let token_conversion = value.checked_mul(&<T::TokenBalance as As<u64>>::sa(selected_sto.rate))
                .ok_or("overflow in calculating tokens")?;

            selected_sto.sold = selected_sto.sold
                .checked_add(&token_conversion)
                .ok_or("overflow while calculating tokens sold")?;

            ensure!(selected_sto.sold <= selected_sto.cap, "There's not enough tokens");

            // Mint tokens and update STO
            T::Asset::_mint_from_sto(ticker.clone(), sender.clone(), token_conversion);

            T::ERC20Trait::transfer(sender, erc20_ticker.clone(), selected_sto.beneficiary.clone(), value)?;

            <StosByToken<T>>::insert((ticker.clone(),sto_id), selected_sto);
            runtime_io::print("Invested in STO");

            Ok(())
        }

    }
}

decl_event!(
    pub enum Event<T>
    where
        AccountId = <T as system::Trait>::AccountId,
    {
        Example(u32, AccountId, AccountId),
        ModifyAllowedTokens(Vec<u8>, Vec<u8>, u32, bool),
    }
);

impl<T: Trait> Module<T> {
    pub fn is_owner(_ticker: Vec<u8>, sender: T::AccountId) -> bool {
        let ticker = utils::bytes_to_upper(_ticker.as_slice());
        T::Asset::is_owner(ticker.clone(), sender)
    }
}

/// tests for this module
#[cfg(test)]
mod tests {
    /*
     *    use super::*;
     *
     *    use primitives::{Blake2Hasher, H256};
     *    use runtime_io::with_externalities;
     *    use runtime_primitives::{
     *        testing::{Digest, DigestItem, Header},
     *        traits::{BlakeTwo256, IdentityLookup},
     *        BuildStorage,
     *    };
     *    use support::{assert_ok, impl_outer_origin};
     *
     *    impl_outer_origin! {
     *        pub enum Origin for Test {}
     *    }
     *
     *    // For testing the module, we construct most of a mock runtime. This means
     *    // first constructing a configuration type (`Test`) which `impl`s each of the
     *    // configuration traits of modules we want to use.
     *    #[derive(Clone, Eq, PartialEq)]
     *    pub struct Test;
     *    impl system::Trait for Test {
     *        type Origin = Origin;
     *        type Index = u64;
     *        type BlockNumber = u64;
     *        type Hash = H256;
     *        type Hashing = BlakeTwo256;
     *        type Digest = Digest;
     *        type AccountId = u64;
     *        type Lookup = IdentityLookup<Self::AccountId>;
     *        type Header = Header;
     *        type Event = ();
     *        type Log = DigestItem;
     *    }
     *    impl Trait for Test {
     *        type Event = ();
     *    }
     *    type TransferValidationModule = Module<Test>;
     *
     *    // This function basically just builds a genesis storage key/value store according to
     *    // our desired mockup.
     *    fn new_test_ext() -> runtime_io::TestExternalities<Blake2Hasher> {
     *        system::GenesisConfig::<Test>::default()
     *            .build_storage()
     *            .unwrap()
     *            .0
     *            .into()
     *    }
     */
}