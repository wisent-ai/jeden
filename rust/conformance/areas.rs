#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct CompletionArea {
    pub(crate) id: &'static str,
    pub(crate) title: &'static str,
    pub(crate) phase: &'static str,
    pub(crate) owner: &'static str,
    pub(crate) acceptance: &'static str,
}

pub(crate) static COMPLETION_AREAS: [CompletionArea; 38] = [
    CompletionArea {
        id: "pelna-macierz-gapow-i-ownership",
        title: "Zapisać pełną macierz gapów Jeden",
        phase: "Wave 0",
        owner: "conformance",
        acceptance: "Manifest zawiera dokładnie 38 unikalnych obszarów z ownerem i bramką akceptacji.",
    },
    CompletionArea {
        id: "mierzalne-kryteria-zamkniecia",
        title: "Zdefiniować mierzalne kryteria zamknięcia",
        phase: "Wave 0",
        owner: "conformance",
        acceptance: "Każdy obszar ma maszynowo czytelny status i niepustą, weryfikowalną bramkę akceptacji.",
    },
    CompletionArea {
        id: "centralny-rejestr-capabilities",
        title: "Wprowadzić centralny rejestr capabilities",
        phase: "Wave 1",
        owner: "capabilities",
        acceptance: "Snapshot rejestru nie ma zduplikowanych identyfikatorów, a UI, narzędzia i approval korzystają z jednego źródła deskryptorów.",
    },
    CompletionArea {
        id: "wersjonowany-graf-sesji",
        title: "Wprowadzić wersjonowany graf sesji",
        phase: "Wave 2A",
        owner: "session",
        acceptance: "Atomowy store odtwarza identyczną projekcję z typowanych zdarzeń i automatycznie migruje poprzedni format.",
    },
    CompletionArea {
        id: "wierne-resume-branch-fork-tree",
        title: "Zapewnić wierne resume branch fork tree",
        phase: "Wave 2A",
        owner: "session",
        acceptance: "Restart, resume, branch, fork i tree zachowują pełną semantykę, lineage i artefakty, a nie tylko tekst rozmowy.",
    },
    CompletionArea {
        id: "trwale-compaction-handoff-checkpoint-i-rewind",
        title: "Utrwalić compaction handoff i rewind",
        phase: "Wave 3A",
        owner: "session",
        acceptance: "Compaction, handoff, checkpoint i rewind są typowanymi operacjami grafu i po restarcie odtwarzają równoważny kontekst.",
    },
    CompletionArea {
        id: "operation-context-i-propagowana-cancellation",
        title: "Wprowadzić operation context i cancellation",
        phase: "Wave 1",
        owner: "runtime",
        acceptance: "Jedno anulowanie przerywa aktywny model request, narzędzia, procesy, MCP, retry i wszystkich potomków operacji.",
    },
    CompletionArea {
        id: "process-manager-pty-i-artifact-sink",
        title: "Wprowadzić process manager i artifact sink",
        phase: "Wave 3B",
        owner: "process",
        acceptance: "Anulowanie usuwa całe drzewo procesów, a pełny output jest zachowany w artifact sink przed opublikowaniem skrótu.",
    },
    CompletionArea {
        id: "dynamiczny-lifecycle-entitlementow-weles",
        title: "Zintegrować dynamiczny auth lifecycle Weles",
        phase: "Wave 2B",
        owner: "control-plane",
        acceptance: "Inspect, begin, poll, cancel i revoke_reference są negocjowane z Weles bez ujawniania ani lokalnego przechowywania sekretów.",
    },
    CompletionArea {
        id: "katalog-modeli-i-tras-brama-wisent",
        title: "Zintegrować katalog modeli Brama",
        phase: "Wave 2B",
        owner: "control-plane",
        acceptance: "Wybór modelu i trasy pochodzi z ważnego snapshotu control plane, a wygasły snapshot daje jawny stan degraded.",
    },
    CompletionArea {
        id: "typowany-streaming-modelu",
        title: "Ujednolicić typowany streaming odpowiedzi",
        phase: "Wave 3C",
        owner: "gateway",
        acceptance: "Fragmentowany stream emituje uporządkowane zdarzenia tekstu, thinking, tools i usage, a malformed dane kończą się typowanym błędem protokołu.",
    },
    CompletionArea {
        id: "retry-failover-i-context-promotion",
        title: "Dodać retry failover i context promotion",
        phase: "Wave 3C",
        owner: "routing",
        acceptance: "Retry respektuje klasyfikację, Retry-After, jitter i cancellation, nie powtarza skutków ubocznych, a promotion poprzedza compaction.",
    },
    CompletionArea {
        id: "context-rules-i-secret-policy",
        title: "Dodać context rules i secret policy",
        phase: "Wave 2C",
        owner: "context",
        acceptance: "Deterministyczne discovery buduje kontekst z provenance i budżetem, a sekrety są chronione przed gateway, exportem, share i artefaktami.",
    },
    CompletionArea {
        id: "unified-read-write-search-resource-semantics",
        title: "Rozbudować read write search semantics",
        phase: "Wave 4A",
        owner: "resources",
        acceptance: "Wspólny router obsługuje typowane selektory, zapis i wyszukiwanie z jail, cancellation, paginacją oraz poprawną invalidacją cache.",
    },
    CompletionArea {
        id: "ast-i-lsp-runtime",
        title: "Dodać AST oraz LSP runtime",
        phase: "Wave 5A",
        owner: "developer-tools",
        acceptance: "AST edit wymaga ważnego preview tokenu, a session-scoped LSP zapewnia diagnostykę, nawigację, refactor, lifecycle i cancellation.",
    },
    CompletionArea {
        id: "persistent-eval-i-terminal-pty",
        title: "Dodać persistent eval i terminal PTY",
        phase: "Wave 5A",
        owner: "eval",
        acceptance: "Kernele zachowują stan między komórkami, wspierają reset i interrupt, a PTY ma zarządzany lifecycle, resize i streaming.",
    },
    CompletionArea {
        id: "browser-debugger-web-github-i-ssh",
        title: "Dodać browser debugger web GitHub SSH",
        phase: "Wave 6A",
        owner: "integrations",
        acceptance: "Każda integracja wykonuje realną operację przez zdrowego providera, propaguje cancellation i nie pokazuje aktywnego UI bez gotowego backendu.",
    },
    CompletionArea {
        id: "image-inspect-generate-i-tts",
        title: "Dodać image inspection generation i TTS",
        phase: "Wave 6A",
        owner: "media",
        acceptance: "Vision, generowanie obrazu i TTS są odrębnymi deskryptorami, respektują modalities oraz zapisują MIME-safe artefakty z usage.",
    },
    CompletionArea {
        id: "pending-actions-checkpoint-resolve-i-rewind",
        title: "Dodać pending actions checkpoint rewind",
        phase: "Wave 4A",
        owner: "session",
        acceptance: "Pending action jest trwałe, apply/discard sprawdza revision i expiry, a checkpoint/rewind operują na tym samym grafie leaf.",
    },
    CompletionArea {
        id: "trwaly-mcp-manager",
        title: "Wprowadzić trwały MCP manager",
        phase: "Wave 4B",
        owner: "mcp",
        acceptance: "Persistent stdio i HTTP/SSE obsługują równoległe requesty, dynamiczne listy, notifications, reconnect z backoff oraz kontrolowany teardown.",
    },
    CompletionArea {
        id: "extension-loader-i-event-bus",
        title: "Wprowadzić extension loader i event bus",
        phase: "Wave 4C",
        owner: "extensions",
        acceptance: "Wersjonowany worker ABI negocjuje capabilities, izoluje crash, respektuje deadline i cancellation oraz publikuje typowane eventy.",
    },
    CompletionArea {
        id: "aktywacja-wszystkich-plugin-capabilities",
        title: "Aktywować wszystkie capabilities pluginów",
        phase: "Wave 4C",
        owner: "plugins",
        acceptance: "Każda zadeklarowana rodzina pluginu jest aktywowana albo ma typowany failure; installed nigdy nie oznacza automatycznie active.",
    },
    CompletionArea {
        id: "skills-rules-i-custom-agents",
        title: "Dodać skills rules i custom agents",
        phase: "Wave 5B",
        owner: "declarative-capabilities",
        acceptance: "Strict schemas, stabilne IDs, scope precedence i bezpieczne resource URI deterministycznie aktywują skills, rules i agents.",
    },
    CompletionArea {
        id: "task-job-scheduler-i-izolacja",
        title: "Wprowadzić task job scheduler i isolation",
        phase: "Wave 5C",
        owner: "tasks",
        acceptance: "Scheduler ogranicza concurrency i recursion, trwale zapisuje wynik, propaguje cancellation oraz izoluje workspace z jawnym merge/capture.",
    },
    CompletionArea {
        id: "agent-communication-i-wspolbieznosc",
        title: "Dodać agent communication i współbieżność",
        phase: "Wave 5C",
        owner: "tasks",
        acceptance: "Mailbox zapewnia trwałe send/inbox/wait/wake z correlation IDs, a narzędzia exclusive nigdy nie wykonują się współbieżnie.",
    },
    CompletionArea {
        id: "autonomiczna-pamiec",
        title: "Wprowadzić autonomiczną pamięć",
        phase: "Wave 6B",
        owner: "memory",
        acceptance: "Worker trwale i idempotentnie ekstrahuje, redaguje, konsoliduje i przywołuje pamięć z lease, heartbeat oraz provenance.",
    },
    CompletionArea {
        id: "pelna-live-collaboration",
        title: "Wprowadzić pełną współpracę live",
        phase: "Wave 6B",
        owner: "collaboration",
        acceptance: "E2EE replika zapewnia reconnect i backfill, rozdziela klucz od prawa zapisu oraz odrzuca nieautoryzowane i replayowane ramki.",
    },
    CompletionArea {
        id: "sdk-rpc-i-acp",
        title: "Dodać SDK RPC i ACP",
        phase: "Wave 1 / Wave 7A",
        owner: "api",
        acceptance: "SDK, skorelowany JSONL RPC i ACP sterują tym samym runtime i emitują równoważne zdarzenia wraz z abort i dispose.",
    },
    CompletionArea {
        id: "odrebny-jezyk-domenowy-i-brand-jeden",
        title: "Zaprojektować odrębny brand Jeden",
        phase: "Wave 7C",
        owner: "product-identity",
        acceptance: "Publiczne typy, komendy, help i dokumentacja używają wyłącznie domeny Jeden/Wisent i są generowane z aktywnego rejestru.",
    },
    CompletionArea {
        id: "pelny-natywny-editor",
        title: "Zbudować pełny natywny edytor",
        phase: "Wave 7B",
        owner: "tui-editor",
        acceptance: "Reducer editora obsługuje grapheme-aware cursor, selection, multiline, undo, history, paste i external editor.",
    },
    CompletionArea {
        id: "bezpieczny-renderer-i-unicode",
        title: "Zbudować bezpieczny renderer Unicode",
        phase: "Wave 7B",
        owner: "tui-renderer",
        acceptance: "Renderer zachowuje scrollback bez duplikacji i strat oraz poprawnie mierzy graphemes, CJK, emoji, ANSI i resize.",
    },
    CompletionArea {
        id: "attachments-inline-images-i-clipboard",
        title: "Dodać attachments obrazy i clipboard",
        phase: "Wave 8A",
        owner: "tui-media",
        acceptance: "Attachments są typowanymi zdarzeniami i artefaktami, a MIME, rozmiar i modality są walidowane przed próbą modelu.",
    },
    CompletionArea {
        id: "steering-follow-up-i-konfigurowalne-skroty",
        title: "Dodać steering followup i skróty",
        phase: "Wave 8A",
        owner: "tui-input",
        acceptance: "Namespaced keymap wykrywa konflikty, a steering i kolejka promptów działają deterministycznie podczas streamu i po cancellation.",
    },
    CompletionArea {
        id: "ui-generowane-z-capability-command-registry",
        title: "Generować UI z capability registry",
        phase: "Wave 8B",
        owner: "tui-registry",
        acceptance: "Każda widoczna komenda ma stabilne ID, schemat argumentów, handler i health-backed enablement z jednego rejestru.",
    },
    CompletionArea {
        id: "themes-accessibility-i-live-status",
        title: "Dodać themes accessibility i status",
        phase: "Wave 8B",
        owner: "tui-presentation",
        acceptance: "Motywy respektują NO_COLOR i niekolorowe sygnały, a status jest żywą projekcją rzeczywistych eventów, kosztów i health.",
    },
    CompletionArea {
        id: "doctor-updater-i-subsystem-health",
        title: "Rozbudować doctor updater i health",
        phase: "Wave 9A",
        owner: "operations",
        acceptance: "Doctor aktywnie sonduje subsystemy ze stabilnym JSON i exit status, a podpisany update wspiera atomic swap, self-test i rollback.",
    },
    CompletionArea {
        id: "automatyczny-conformance-reliability-system",
        title: "Dodać automatyczny conformance system",
        phase: "Wave 9B",
        owner: "conformance",
        acceptance: "Automatyczne scenariusze fault, protocol, PTY i migration pokrywają sukces, błąd, cancellation, restart oraz prohibited symbols.",
    },
    CompletionArea {
        id: "usuniecie-ui-only-no-op-dead-paths",
        title: "Usunąć wszystkie UI-only i no-op ścieżki",
        phase: "Wave 9B",
        owner: "clean-cutover",
        acceptance: "Repo nie zawiera atrap ani martwych ścieżek, a każda aktywna powierzchnia ma wykonywalny handler i health-backed capability.",
    },
];

pub(crate) fn completion_areas() -> &'static [CompletionArea] {
    &COMPLETION_AREAS
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct ProductionScope {
    pub(crate) id: &'static str,
    pub(crate) check_id: &'static str,
    pub(crate) owner: &'static str,
    pub(crate) fixture: &'static str,
    pub(crate) artifact_path: &'static str,
}

macro_rules! scope {
    ($id:literal, $owner:literal) => {
        ProductionScope {
            id: $id,
            check_id: concat!("production/", $id, "/behavior"),
            owner: $owner,
            fixture: concat!("tests/conformance/contracts/fixtures/", $id, ".json"),
            artifact_path: concat!(".jeden/conformance/artifacts/", $id, ".json"),
        }
    };
}

pub(crate) static PRODUCTION_SCOPES: [ProductionScope; 23] = [
    scope!("01-realne-brama-weles-e2e", "control-plane"),
    scope!("02-podpisany-release-engineering", "release"),
    scope!("03-kontraktowe-ci-i-migracje", "migrations"),
    scope!("04-sandbox-i-security-audit", "runtime-security"),
    scope!("05-dlugotrwale-reliability-tests", "reliability"),
    scope!("06-private-opentelemetry", "telemetry"),
    scope!("07-coding-agent-benchmark", "quality"),
    scope!("08-outcome-based-routing", "routing"),
    scope!("09-semantic-quality-memory", "memory"),
    scope!("10-ide-acp-integration", "protocol"),
    scope!("11-stable-rust-typescript-python-sdk", "sdk"),
    scope!("12-secure-headless-service", "headless"),
    scope!("13-production-signed-marketplace", "marketplace"),
    scope!("14-remote-worker-pool", "workers"),
    scope!("15-multiplatform", "platform"),
    scope!("16-staging-brama-weles", "control-plane"),
    scope!("17-nightly-all-interface-e2e", "e2e"),
    scope!("18-crash-fault-matrix", "reliability"),
    scope!("19-conformance-ci-gate", "conformance"),
    scope!("20-signed-canary-rollback", "release"),
    scope!("21-representative-benchmark-run", "quality"),
    scope!("22-warning-debt-deny-warnings", "quality"),
    scope!("23-quality-reliability-report", "release"),
];

pub(crate) fn production_scopes() -> &'static [ProductionScope] {
    &PRODUCTION_SCOPES
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    #[test]
    fn conformance_registry_invariants() {
        assert_eq!(COMPLETION_AREAS.len(), 38);

        let mut ids = HashSet::new();
        let mut titles = HashSet::new();
        for area in COMPLETION_AREAS {
            assert!(ids.insert(area.id), "duplicate area id: {}", area.id);
            assert!(
                titles.insert(area.title),
                "duplicate area title: {}",
                area.title
            );
            assert!(
                !area.id.is_empty()
                    && !area.id.starts_with('-')
                    && !area.id.ends_with('-')
                    && !area.id.contains("--")
                    && area.id.bytes().all(|byte| byte.is_ascii_lowercase()
                        || byte.is_ascii_digit()
                        || byte == b'-'),
                "area id is not kebab-case: {}",
                area.id
            );
            assert!(!area.phase.trim().is_empty(), "empty phase for {}", area.id);
            assert!(!area.owner.trim().is_empty(), "empty owner for {}", area.id);
            assert!(
                !area.acceptance.trim().is_empty(),
                "empty acceptance for {}",
                area.id
            );
        }
    }

    #[test]
    fn conformance_registry_has_exact_title_set() {
        let mut actual = COMPLETION_AREAS
            .iter()
            .map(|area| area.title)
            .collect::<Vec<_>>();
        let mut expected = vec![
            "Zapisać pełną macierz gapów Jeden",
            "Zdefiniować mierzalne kryteria zamknięcia",
            "Wprowadzić centralny rejestr capabilities",
            "Wprowadzić wersjonowany graf sesji",
            "Zapewnić wierne resume branch fork tree",
            "Utrwalić compaction handoff i rewind",
            "Wprowadzić operation context i cancellation",
            "Wprowadzić process manager i artifact sink",
            "Zintegrować dynamiczny auth lifecycle Weles",
            "Zintegrować katalog modeli Brama",
            "Ujednolicić typowany streaming odpowiedzi",
            "Dodać retry failover i context promotion",
            "Dodać context rules i secret policy",
            "Rozbudować read write search semantics",
            "Dodać AST oraz LSP runtime",
            "Dodać persistent eval i terminal PTY",
            "Dodać browser debugger web GitHub SSH",
            "Dodać image inspection generation i TTS",
            "Dodać pending actions checkpoint rewind",
            "Wprowadzić trwały MCP manager",
            "Wprowadzić extension loader i event bus",
            "Aktywować wszystkie capabilities pluginów",
            "Dodać skills rules i custom agents",
            "Wprowadzić task job scheduler i isolation",
            "Dodać agent communication i współbieżność",
            "Wprowadzić autonomiczną pamięć",
            "Wprowadzić pełną współpracę live",
            "Dodać SDK RPC i ACP",
            "Zaprojektować odrębny brand Jeden",
            "Zbudować pełny natywny edytor",
            "Zbudować bezpieczny renderer Unicode",
            "Dodać attachments obrazy i clipboard",
            "Dodać steering followup i skróty",
            "Generować UI z capability registry",
            "Dodać themes accessibility i status",
            "Rozbudować doctor updater i health",
            "Dodać automatyczny conformance system",
            "Usunąć wszystkie UI-only i no-op ścieżki",
        ];
        actual.sort_unstable();
        expected.sort_unstable();

        assert_eq!(actual, expected);
    }

    #[test]
    fn production_scope_registry_has_23_machine_readable_gates() {
        assert_eq!(PRODUCTION_SCOPES.len(), 23);
        let mut ids = HashSet::new();
        let mut check_ids = HashSet::new();
        for scope in PRODUCTION_SCOPES {
            assert!(
                ids.insert(scope.id),
                "duplicate production scope {}",
                scope.id
            );
            assert!(
                check_ids.insert(scope.check_id),
                "duplicate production check {}",
                scope.check_id
            );
            assert!(
                scope.check_id.starts_with("production/") && scope.check_id.ends_with("/behavior")
            );
            assert!(
                !scope.owner.is_empty()
                    && !scope.fixture.is_empty()
                    && !scope.artifact_path.is_empty()
            );
        }
    }
}
