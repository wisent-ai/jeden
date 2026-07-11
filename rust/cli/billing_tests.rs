use super::billing::{billing_slash_handlers, parse_billing_command};
use std::collections::BTreeSet;

#[test]
fn every_registered_billing_slash_has_exactly_one_live_parser_handler() {
    let handlers = billing_slash_handlers();
    let expected = [
        ("/payment-method setup", "payment-method.setup"),
        ("/billing policy get", "billing.policy.get"),
        ("/billing policy set", "billing.policy.set"),
        ("/billing policy reset", "billing.policy.reset"),
        ("/subscriptions list", "subscriptions.list"),
        ("/subscriptions status", "subscriptions.status"),
        ("/subscriptions disable", "subscriptions.disable"),
        ("/subscriptions purchase", "subscriptions.purchase"),
        ("/subscriptions renew", "subscriptions.renew"),
    ];
    assert_eq!(handlers, expected);

    let unique_commands = handlers
        .iter()
        .map(|(command, _)| *command)
        .collect::<BTreeSet<_>>();
    let unique_handlers = handlers
        .iter()
        .map(|(_, handler)| *handler)
        .collect::<BTreeSet<_>>();
    assert_eq!(
        unique_commands.len(),
        handlers.len(),
        "duplicate billing slash registration"
    );
    assert_eq!(
        unique_handlers.len(),
        handlers.len(),
        "multiple slash commands share a mutation handler"
    );

    for (command, _) in handlers {
        let input = match *command {
            "/payment-method setup" => "/payment-method setup --account acct_exact",
            "/billing policy get" => "/billing policy get --account acct_exact",
            "/billing policy set" => "/billing policy set --account acct_exact --enabled --products pro --currencies USD --max-single 100 --max-period 200 --revision rev_exact --valid-until 4102444800000 --approve",
            "/billing policy reset" => "/billing policy reset --account acct_exact --approve",
            "/subscriptions list" => "/subscriptions list --account acct_exact",
            "/subscriptions status" => "/subscriptions status --account acct_exact --subscription sub_exact",
            "/subscriptions disable" => "/subscriptions disable --account acct_exact --subscription sub_exact --idempotency disable_1 --approve",
            "/subscriptions purchase" => "/subscriptions purchase --account acct_exact --product pro --currency USD --idempotency purchase_1 --approve",
            "/subscriptions renew" => "/subscriptions renew --account acct_exact --subscription sub_exact --idempotency renew_1 --approve",
            other => panic!("uncovered billing handler {other}"),
        };
        assert!(
            parse_billing_command(input).is_ok(),
            "registered command has no parser handler: {input}"
        );
    }
}

#[test]
fn sensitive_payment_input_is_not_part_of_any_registered_command_surface() {
    let registry = format!("{:#?}", billing_slash_handlers()).to_ascii_lowercase();
    for forbidden in [
        "pan",
        "cvv",
        "cvc",
        "card-number",
        "processor-token",
        "payment-token",
        "address",
    ] {
        assert!(
            !registry.contains(forbidden),
            "billing registry exposed forbidden payment field {forbidden}"
        );
    }
}
