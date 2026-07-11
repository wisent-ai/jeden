use super::manifest::{PluginDependency, PluginReleaseV1};
use semver::{Version, VersionReq};
use std::collections::{BTreeMap, BTreeSet};

#[derive(Clone, Debug)]
struct State<'a> {
    selected: BTreeMap<String, &'a PluginReleaseV1>,
    constraints: BTreeMap<String, Vec<VersionReq>>,
    requested_features: BTreeMap<String, BTreeSet<String>>,
}

fn candidates<'a>(
    id: &str,
    state: &State<'a>,
    releases: &'a [PluginReleaseV1],
    platform: &str,
) -> Result<Vec<&'a PluginReleaseV1>, String> {
    let requirements = state.constraints.get(id).cloned().unwrap_or_default();
    let features = state
        .requested_features
        .get(id)
        .cloned()
        .unwrap_or_default();
    let mut matches = releases
        .iter()
        .filter(|release| release.id == id)
        .filter_map(|release| {
            let version = Version::parse(&release.version).ok()?;
            (requirements
                .iter()
                .all(|requirement| requirement.matches(&version))
                && features.is_subset(&release.features)
                && (release.platforms.is_empty() || release.platforms.contains(platform)))
            .then_some((version, release))
        })
        .collect::<Vec<_>>();
    matches.sort_by(|(a, _), (b, _)| b.cmp(a));
    if matches.is_empty() {
        return Err(format!(
            "no release of {id} satisfies all version, feature, and platform constraints"
        ));
    }
    Ok(matches.into_iter().map(|(_, release)| release).collect())
}

fn add_dependency(state: &mut State<'_>, dependency: &PluginDependency) -> Result<(), String> {
    if dependency.optional && dependency.features.is_empty() {
        return Ok(());
    }
    let requirement = VersionReq::parse(&dependency.requirement)
        .map_err(|error| format!("invalid requirement for {}: {error}", dependency.id))?;
    state
        .constraints
        .entry(dependency.id.clone())
        .or_default()
        .push(requirement);
    state
        .requested_features
        .entry(dependency.id.clone())
        .or_default()
        .extend(dependency.features.iter().cloned());
    Ok(())
}

fn solve<'a>(
    state: State<'a>,
    releases: &'a [PluginReleaseV1],
    platform: &str,
) -> Result<State<'a>, String> {
    for (id, selected) in &state.selected {
        let version = Version::parse(&selected.version).map_err(|error| error.to_string())?;
        if state.constraints.get(id).is_some_and(|requirements| {
            requirements
                .iter()
                .any(|requirement| !requirement.matches(&version))
        }) {
            return Err(format!(
                "selected {id}@{} conflicts with a transitive constraint",
                selected.version
            ));
        }
    }
    let unresolved = state
        .constraints
        .keys()
        .find(|id| !state.selected.contains_key(*id))
        .cloned();
    let Some(id) = unresolved else {
        return Ok(state);
    };
    let choices = candidates(&id, &state, releases, platform)?;
    let mut failures = Vec::new();
    for release in choices {
        let mut branch = state.clone();
        branch.selected.insert(id.clone(), release);
        let mut valid = true;
        for dependency in &release.dependencies {
            if let Err(error) = add_dependency(&mut branch, dependency) {
                failures.push(error);
                valid = false;
                break;
            }
        }
        if valid {
            match solve(branch, releases, platform) {
                Ok(solution) => return Ok(solution),
                Err(error) => failures.push(error),
            }
        }
    }
    Err(format!("cannot resolve {id}: {}", failures.join("; ")))
}

fn reject_cycles(selected: &BTreeMap<String, &PluginReleaseV1>) -> Result<(), String> {
    fn visit(
        id: &str,
        selected: &BTreeMap<String, &PluginReleaseV1>,
        visiting: &mut BTreeSet<String>,
        visited: &mut BTreeSet<String>,
    ) -> Result<(), String> {
        if visited.contains(id) {
            return Ok(());
        }
        if !visiting.insert(id.to_string()) {
            return Err(format!("dependency cycle contains {id}"));
        }
        if let Some(release) = selected.get(id) {
            for dependency in &release.dependencies {
                if selected.contains_key(&dependency.id) {
                    visit(&dependency.id, selected, visiting, visited)?;
                }
            }
        }
        visiting.remove(id);
        visited.insert(id.to_string());
        Ok(())
    }
    let mut visiting = BTreeSet::new();
    let mut visited = BTreeSet::new();
    for id in selected.keys() {
        visit(id, selected, &mut visiting, &mut visited)?;
    }
    Ok(())
}

pub fn resolve<'a>(
    roots: &[PluginDependency],
    releases: &'a [PluginReleaseV1],
    platform: &str,
) -> Result<Vec<&'a PluginReleaseV1>, String> {
    let mut state = State {
        selected: BTreeMap::new(),
        constraints: BTreeMap::new(),
        requested_features: BTreeMap::new(),
    };
    for root in roots {
        add_dependency(&mut state, root)?;
    }
    let solved = solve(state, releases, platform)?;
    reject_cycles(&solved.selected)?;
    Ok(solved.selected.into_values().collect())
}
