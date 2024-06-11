use std::{env, fs::File, io::Write, path::PathBuf};

use proc_macro2::{Span, TokenStream};
use quote::{quote, ToTokens};
use solores::idl_format::anchor::{
    accounts::NamedAccount,
    instructions::{to_ix_accounts, NamedInstruction},
    typedefs::{NamedType, TypedefType},
    AnchorIdl,
};
use syn::Ident;

const ACCOUNT_WANTED: &[&str] = &["Pool", "Custody", "CustomOracle"];
const TYPE_WANTED: &[&str] = &[
    "TokenRatios",
    "OracleType",
    "OracleParams",
    "PricingParams",
    "Permissions",
    "Fees",
    "BorrowRateParams",
    "Assets",
    "FeesStats",
    "VolumeStats",
    "TradeStats",
    "PositionStats",
    "BorrowRateState",
    "StableLockedAmountStat",
    "PositionsAccounting",
    "U128Split",
    "LimitedString",
    "SwapParams",
];
const INSTRUCTIONS_WANTED: &[&str] = &["swap"];

fn generate_type_def(name: &Ident, def: &TypedefType) -> TokenStream {
    match def {
        TypedefType::r#struct(def_struct) => {
            let fields = def_struct.into_token_stream();
            quote! {
                pub struct #name {
                    #fields
                }
            }
        }
        TypedefType::r#enum(def_enum) => {
            let variants = &def_enum.variants;
            quote! {
                pub enum #name {
                    #(#variants),*
                }
            }
        }
    }
}

pub struct AnchorType(NamedType);

impl ToTokens for AnchorType {
    fn to_tokens(&self, tokens: &mut TokenStream) {
        let name = Ident::new(&self.0.name, Span::call_site());
        let def = generate_type_def(&name, &self.0.r#type);

        let final_tokens = quote! {
            #[derive(BorshDeserialize, BorshSerialize, Debug, Clone)]
            #def
        };

        final_tokens.to_tokens(tokens)
    }
}

pub struct AnchorInstruction(NamedInstruction);

impl ToTokens for AnchorInstruction {
    fn to_tokens(&self, tokens: &mut TokenStream) {
        let Some(accounts) = &self.0.accounts else {
            return;
        };

        let ins_accounts = to_ix_accounts(&accounts);

        self.0.write_keys_struct(tokens, &ins_accounts);
        self.0.write_accounts_len(tokens, ins_accounts.len());
        self.0.write_from_keys_for_meta_arr(tokens, &ins_accounts);
    }
}

pub struct AnchorAccount(NamedAccount);

impl AnchorAccount {
    fn get_named_type(&self) -> &NamedType {
        &self.0 .0
    }
}

impl ToTokens for AnchorAccount {
    fn to_tokens(&self, tokens: &mut TokenStream) {
        let named_type = self.get_named_type();
        let account_name_str = &named_type.name;
        let account_name = Ident::new(&named_type.name, Span::call_site());
        let def = generate_type_def(&account_name, &named_type.r#type);
        let discriminator: proc_macro2::TokenStream = {
            let discriminator_preimage = format!("account:{}", account_name_str);
            let mut discriminator = [0u8; 8];
            discriminator.copy_from_slice(
                &anchor_syn::hash::hash(discriminator_preimage.as_bytes()).to_bytes()[..8],
            );
            format!("{discriminator:?}").parse().unwrap()
        };

        let final_tokens = quote! {
            #[derive(BorshDeserialize, BorshSerialize, Debug, Clone)]
            #def

            #[automatically_derived]
            impl anchor_lang::Discriminator for #account_name {
                const DISCRIMINATOR: [u8; 8] = #discriminator;
            }

            #[automatically_derived]
            impl anchor_lang::AccountDeserialize for #account_name {
                fn try_deserialize(buf: &mut &[u8]) -> anchor_lang::Result<Self> {
                    if buf.len() < #discriminator.len() {
                        return Err(anchor_lang::error::ErrorCode::AccountDiscriminatorNotFound.into());
                    }
                    let given_disc = &buf[..8];
                    if &#discriminator != given_disc {
                        return Err(anchor_lang::error!(anchor_lang::error::ErrorCode::AccountDiscriminatorMismatch).with_account_name(#account_name_str));
                    }
                    Self::try_deserialize_unchecked(buf)
                }

                fn try_deserialize_unchecked(buf: &mut &[u8]) -> anchor_lang::Result<Self> {
                    let mut data: &[u8] = &buf[8..];
                    AnchorDeserialize::deserialize(&mut data)
                        .map_err(|_| anchor_lang::error::ErrorCode::AccountDidNotDeserialize.into())
                }
            }
        };

        final_tokens.to_tokens(tokens)
    }
}

fn main() -> anyhow::Result<()> {
    let idl_file = File::open("idl/adrena.json")?;

    let idl: AnchorIdl = serde_json::from_reader(idl_file)?;

    let mut final_tokens = quote! {
        use anchor_lang::prelude::*;
        use anchor_lang::prelude::borsh::BorshDeserialize;
        use anchor_lang::prelude::borsh::BorshSerialize;
    };

    // Writes accounts
    if let Some(idl_accounts) = idl.accounts {
        let accounts = idl_accounts.into_iter().filter_map(|acc| {
            if ACCOUNT_WANTED.contains(&acc.0.name.as_str()) {
                Some(AnchorAccount(acc).into_token_stream())
            } else {
                None
            }
        });

        let token = quote! {
            #(#accounts)*
        };
        token.to_tokens(&mut final_tokens);
    }

    // Writes custom types
    if let Some(idl_types) = idl.types {
        let types = idl_types.into_iter().filter_map(|ty| {
            if TYPE_WANTED.contains(&ty.name.as_str()) {
                Some(AnchorType(ty).into_token_stream())
            } else {
                None
            }
        });

        let token = quote! {
            #(#types)*
        };
        token.to_tokens(&mut final_tokens);
    }

    if let Some(idl_instructions) = idl.instructions {
        let instructions = idl_instructions.into_iter().filter_map(|ins| {
            if INSTRUCTIONS_WANTED.contains(&ins.name.as_str()) {
                Some(AnchorInstruction(ins).to_token_stream())
            } else {
                None
            }
        });

        let token = quote! {
            #(#instructions)*
        };
        token.to_tokens(&mut final_tokens);
    }

    let out_path = PathBuf::from(env::var("OUT_DIR").unwrap());
    let mut file = File::create(out_path.join("adrena.rs"))?;

    write!(file, "{final_tokens}")?;

    Ok(())
}
