//! Jeden's default communication contract: how an answer to the user is
//! shaped when the operator has not written their own.
//!
//! `contracts.communication` resolves in three ways. Unset, or set to the
//! empty string, means this default applies. The literal value `none` turns the
//! default off and adds no communication instruction. Any other text replaces
//! the default with the operator's own contract.

use crate::cli::config::UiLanguage;

/// The `contracts.communication` value that disables the default contract.
pub(crate) const NONE: &str = "none";

/// The default contract in the prompt language.
pub(crate) fn default_text(language: &UiLanguage) -> &'static str {
    const EN: &str = "Write to the user in plain language: short, ordinary sentences a person would say out loud, with a subject and a verb. No technical jargon where a plain word exists, no filler, no repeating the question, no summary at the end. Every reply contains only what the user's latest message asks for; nothing about hooks, guards, task ledgers, verification runs or earlier turns unless the user asked about them. A question gets its answer, first and without headings. A report of work you carried out has three parts, in this order, each under its own heading. \"What was done\": what happened, to what, and what came out of it, in the past tense, with the real names, paths, commands and numbers. \"Blockers\": what stopped or limited the work, each with the exact error or refusal and what was tried; write \"none\" if nothing did. \"Next steps\": only what the user has to do or decide, or what somebody outside this machine has to do; write \"none\" if there is nothing. Work you can do yourself never belongs there: naming it instead of doing it does not end the turn, and an answer that leaves such work is not an answer yet. A guard or hook that interrupts a turn is settled with tool calls, not with a new report: a report already given is never sent again.";
    const PL: &str = "Pisz do użytkownika prostym językiem: krótkie, zwykłe zdania, jakie ktoś powiedziałby na głos, z podmiotem i czasownikiem. Bez technicznego żargonu tam, gdzie jest zwykłe słowo, bez wypełniaczy, bez powtarzania pytania, bez podsumowania na końcu. Każda odpowiedź zawiera wyłącznie to, o co pyta ostatnia wiadomość użytkownika; nic o hookach, strażnikach, rejestrach zadań, weryfikacjach ani wcześniejszych turach, chyba że użytkownik o nie zapytał. Pytanie dostaje odpowiedź, na początku i bez nagłówków. Raport z wykonanej pracy ma trzy części, w tej kolejności, każda pod własnym nagłówkiem. „Co zostało zrobione”: co się stało, z czym i co z tego wyszło, w czasie przeszłym, z prawdziwymi nazwami, ścieżkami, poleceniami i liczbami. „Blokery”: co zatrzymało lub ograniczyło pracę, każdy z dokładnym błędem lub odmową i tym, co próbowano; napisz „brak”, jeśli nic. „Następne kroki”: wyłącznie to, co użytkownik ma zrobić lub zdecydować, albo co ma zrobić ktoś poza tą maszyną; napisz „brak”, jeśli nic nie zostało. Praca, którą możesz wykonać sam, nigdy tam nie należy: nazwanie jej zamiast wykonania nie kończy tury, a odpowiedź, która ją zostawia, nie jest jeszcze odpowiedzią. Strażnika lub hooka, który przerywa turę, obsługujesz wywołaniami narzędzi, a nie nowym raportem: raportu już podanego nigdy nie wysyłasz ponownie.";
    if language.code() == "pl" {
        PL
    } else {
        EN
    }
}

/// How a configured value resolves.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Source {
    /// Nothing configured: Jeden's default contract applies.
    Default,
    /// The operator wrote their own contract.
    Operator,
    /// The operator set `none`: no communication contract at all.
    Disabled,
}

impl Source {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::Default => "default",
            Self::Operator => "operator",
            Self::Disabled => "disabled",
        }
    }
}

/// The contract text in force for `configured`, and where it came from.
pub(crate) fn resolve<'a>(configured: &'a str, language: &UiLanguage) -> (Source, &'a str)
where
    'static: 'a,
{
    let configured = configured.trim();
    if configured.is_empty() {
        (Source::Default, default_text(language))
    } else if configured.eq_ignore_ascii_case(NONE) {
        (Source::Disabled, "")
    } else {
        (Source::Operator, configured)
    }
}
