//! One task contract for prompts, RPC settings, durable reports and diagnostics.
//! Report validation checks completeness, not the truth of model claims.

use crate::cli::config::UiLanguage;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::fmt::Write;

pub(crate) const VERSION: u32 = 1;
pub(crate) const VIOLATION_EVENT: &str = "contract_violation";
pub(crate) const REPAIR_INSTRUCTION: &str = "Return the complete final action with text and report. Report must contain functionality, diagnostics, cli, gui, documentation, tests, delivery. Each entry must have status (done, not_applicable, or blocked), a concrete nonempty explanation of how you fulfilled it or why it does not apply/is blocked, and an evidence array. Every done entry needs at least one nonempty evidence reference. Do not invent work or results, repeat completed work, or mark unfinished work done. A heading alone is not a report. The next invalid report ends this turn with a contract error, not success.";

struct Requirement {
    id: &'static str,
    en: (&'static str, &'static str),
    pl: (&'static str, &'static str),
}

const REQUIREMENTS: [Requirement; 7] = [
    Requirement {
        id: "functionality",
        en: ("Functionality", "Build or repair reusable product functionality at the source of the defect. Explain the changed behavior and why it works for subsequent inputs, not just this incident."),
        pl: ("Funkcjonalność", "Zbuduj lub napraw funkcjonalność wielokrotnego użytku u źródła usterki. Opisz zmianę zachowania i dlaczego działa dla kolejnych przypadków, a nie tylko tego jednego."),
    },
    Requirement {
        id: "diagnostics",
        en: ("Diagnostics", "Build or repair product diagnostics that expose the actual failed operation, cause and observed state. A configuration declaration, nonempty field or healthy process is not proof that its consumer works."),
        pl: ("Diagnostyka", "Zbuduj lub napraw diagnostykę produktu pokazującą rzeczywistą nieudaną operację, przyczynę i zaobserwowany stan. Deklaracja konfiguracji, niepuste pole ani działający proces nie dowodzą, że ich odbiorca działa."),
    },
    Requirement {
        id: "cli",
        en: ("CLI", "Deliver the capability through the product's CLI contract, including useful refusals and diagnostics. Name the commands and observed final state."),
        pl: ("CLI", "Udostępnij funkcjonalność zgodnie z kontraktem CLI produktu, razem z czytelnymi odmowami i diagnostyką. Podaj polecenia i zaobserwowany stan końcowy."),
    },
    Requirement {
        id: "gui",
        en: ("GUI", "Deliver the same capability and diagnostics on every applicable graphical surface. CLI and GUI have parity; a missing GUI implementation is unfinished work. Name the screens and what was actually observed."),
        pl: ("GUI", "Udostępnij tę samą funkcjonalność i diagnostykę we wszystkich właściwych interfejsach graficznych. GUI ma te same możliwości co CLI; brak implementacji GUI oznacza nieukończoną pracę. Podaj ekrany i to, co rzeczywiście zaobserwowano."),
    },
    Requirement {
        id: "documentation",
        en: ("Documentation", "Update the product's canonical documentation in the same change, including commands, examples, refusals and diagnostic interpretation. Check it against the implementation and name the updated pages."),
        pl: ("Dokumentacja", "Zaktualizuj oficjalną dokumentację produktu w tej samej zmianie: polecenia, przykłady, odmowy i interpretację diagnostyki. Sprawdź zgodność z implementacją i podaj zmienione strony."),
    },
    Requirement {
        id: "tests",
        en: ("Real tests", "Keep tests in the product repository under tests/<area>/ and run them through Probierz. Execute the actual product through its real interface with real dependencies and inspect persisted or external final state. A fleet lifecycle test creates, edits and deletes a real isolated fleet through the CLI. Cover success and meaningful refusals; retain the exact source revision, commands, exit statuses, reports and supported recordings. Mocks, stubs, canned responses, simulated providers, dry runs, compilation and schema checks do not prove the feature. If the real flow cannot run, report blocked, never passed."),
        pl: ("Realne testy", "Testy umieść w repozytorium produktu w tests/<obszar>/ i uruchom przez Probierz. Wykonaj prawdziwy produkt przez jego rzeczywisty interfejs z prawdziwymi zależnościami; sprawdź zapisany lub zewnętrzny stan końcowy. Test cyklu życia floty tworzy, edytuje i usuwa prawdziwą odizolowaną flotę przez CLI. Sprawdź powodzenie i istotne odmowy; zachowaj dokładną rewizję źródeł, polecenia, kody wyjścia, raporty i obsługiwane nagrania. Mocki, stuby, gotowe odpowiedzi, symulowani dostawcy, dry run, kompilacja i sprawdzenie schematu nie dowodzą działania funkcji. Jeśli prawdziwego przebiegu nie da się uruchomić, zgłoś blokadę, nigdy powodzenie."),
    },
    Requirement {
        id: "delivery",
        en: ("Delivery", "Remove superseded paths, commit and push the change, and identify the delivered revision. Explain any actual external blocker without calling the task complete."),
        pl: ("Dostarczenie", "Usuń zastąpione ścieżki, zacommituj i wypchnij zmianę oraz podaj dostarczoną rewizję. Opisz rzeczywistą zewnętrzną blokadę, nie nazywając zadania ukończonym."),
    },
];

impl Requirement {
    fn prose(&self, language: &UiLanguage) -> (&'static str, &'static str) {
        if language.code() == "pl" {
            self.pl
        } else {
            self.en
        }
    }
}

pub(crate) fn section(language: &UiLanguage) -> String {
    let mut text = if language.code() == "pl" {
        "Kontrakt zadania:\nZadanie obejmuje trwałe zachowanie produktu, nie jednorazową naprawę. Nie zastępuj funkcjonalności ręcznym restartem ani skryptem inline. Naprawiaj usterki związane z przydzielonym zadaniem; nie rozszerzaj go na niezwiązane obserwacje. Przed implementacją ustal kryteria ukończenia i właściwe interfejsy na podstawie kontraktu produktu. Każdy punkt wymaga opisu wykonania. Pytanie, czytanie lub plan nie upoważniają do zmian: wyjaśnij wtedy, dlaczego dany punkt nie dotyczy zadania. Ograniczenia użytkownika i uprawnień nadal obowiązują.\n".to_string()
    } else {
        "Task contract:\nDeliver durable product behavior, not a one-time repair. Never substitute a manual restart or inline script for a product capability. Fix defects related to the assigned task, not unrelated observations. Establish completion criteria and applicable surfaces from the product contract before implementation. Explain every requirement. A question, reading request or plan does not authorize changes: explain why the corresponding requirements do not apply. User scope restrictions and tool permissions still apply.\n".to_string()
    };
    for requirement in &REQUIREMENTS {
        let (title, description) = requirement.prose(language);
        writeln!(text, "- {} ({title}): {description}", requirement.id).unwrap();
    }
    text
}

pub(crate) fn snapshot(language: &UiLanguage) -> Value {
    json!({
        "version": VERSION,
        "instructions": section(language),
        "requirements": REQUIREMENTS.iter().map(|requirement| {
            let (title, description) = requirement.prose(language);
            json!({ "id": requirement.id, "title": title, "description": description })
        }).collect::<Vec<_>>(),
    })
}

pub(crate) fn turn_instruction() -> &'static str {
    "Task delivery contract v1 applies to this turn, including tasks completed without tools. Return {\"action\":\"final\",\"text\":\"your concise answer\",\"report\":{...}}. The report contains exactly functionality, diagnostics, cli, gui, documentation, tests, delivery. Each entry has {\"status\":\"done|not_applicable|blocked\",\"explanation\":\"how this requirement was fulfilled, or the concrete reason it does not apply/is blocked\",\"evidence\":[\"actual file, command, run, revision or other source reference\"]}. Use one of the three status values, not their combined spelling. A done entry requires evidence; other statuses may use an empty array. Write explanations in the user's language. Do not put a second report in text: Jeden renders the report. Do not mark an applicable but unfinished requirement not_applicable. Tests for product changes must execute real full flows and retain results through Probierz. Do not fabricate tests, results or evidence. A report describes your work; its structural acceptance is not independent verification of your claims."
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct DeliveryReport {
    functionality: ReportEntry,
    diagnostics: ReportEntry,
    cli: ReportEntry,
    gui: ReportEntry,
    documentation: ReportEntry,
    tests: ReportEntry,
    delivery: ReportEntry,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct ReportEntry {
    status: ReportStatus,
    explanation: String,
    evidence: Vec<String>,
}

#[derive(Debug, PartialEq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
enum ReportStatus {
    Done,
    NotApplicable,
    Blocked,
}

impl DeliveryReport {
    fn entries(&self) -> [&ReportEntry; 7] {
        [
            &self.functionality,
            &self.diagnostics,
            &self.cli,
            &self.gui,
            &self.documentation,
            &self.tests,
            &self.delivery,
        ]
    }

    pub(crate) fn parse(value: Option<Value>) -> Result<Self, String> {
        let value = value.ok_or("final action requires report for every task requirement")?;
        let report: Self = serde_json::from_value(value)
            .map_err(|error| format!("invalid task report: {error}"))?;
        for (requirement, entry) in REQUIREMENTS.iter().zip(report.entries()) {
            if entry.explanation.trim().is_empty() {
                return Err(format!("report.{}.explanation must describe how it was done or why it does not apply/is blocked", requirement.id));
            }
            if entry
                .evidence
                .iter()
                .any(|reference| reference.trim().is_empty())
            {
                return Err(format!(
                    "report.{}.evidence contains an empty reference",
                    requirement.id
                ));
            }
            if entry.status == ReportStatus::Done && entry.evidence.is_empty() {
                return Err(format!(
                    "report.{}.evidence is required for done",
                    requirement.id
                ));
            }
        }
        Ok(report)
    }

    pub(crate) fn is_blocked(&self) -> bool {
        self.entries()
            .iter()
            .any(|entry| entry.status == ReportStatus::Blocked)
    }

    pub(crate) fn render(&self, language: &UiLanguage) -> String {
        let polish = language.code() == "pl";
        let mut text = if polish {
            "Jak to zostało zrobione\n"
        } else {
            "How it was done\n"
        }
        .to_string();
        for (requirement, entry) in REQUIREMENTS.iter().zip(self.entries()) {
            let (title, _) = requirement.prose(language);
            let status = match (&entry.status, polish) {
                (ReportStatus::Done, true) => "wykonano",
                (ReportStatus::NotApplicable, true) => "nie dotyczy",
                (ReportStatus::Blocked, true) => "blokada",
                (ReportStatus::Done, false) => "done",
                (ReportStatus::NotApplicable, false) => "not applicable",
                (ReportStatus::Blocked, false) => "blocked",
            };
            writeln!(text, "{title} ({status}): {}", entry.explanation.trim()).unwrap();
            if !entry.evidence.is_empty() {
                writeln!(
                    text,
                    "{}: {}",
                    if polish { "Dowody" } else { "Evidence" },
                    entry.evidence.join("; ")
                )
                .unwrap();
            }
        }
        text.trim_end().to_string()
    }
}
