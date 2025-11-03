#![cfg(feature = "localnet")]

feature_groups! {
    "batch_all";
    "batch1" {
        mod liquidate;
        mod lookup_table;
        mod transfer_fee;
        mod multisig;
        mod liquidate_fee;
    }
    "batch2" {
        mod load;
        mod pool_overpayment;
        mod rounding;
        mod sanity;
        mod pools;
        mod withdraw_fees;
    }
}

macro_rules! feature_groups {
    (
		$parent:literal;
		$($group_name:literal {
			$(mod $mod_name:ident;)*
		})*
	) => {
        $($(
			#[cfg(any(feature = $parent, feature = $group_name))]
			mod $mod_name;
		)*)*
    };
}
use feature_groups;
