//! The check the writings tests of both facades run: each input rewritten
//! in each writing, the operations run on the original and on the rewrite,
//! what they give compared, and the differences matched against the strict
//! list of known ones.

use std::collections::{BTreeMap, BTreeSet};
use std::hash::{DefaultHasher, Hash, Hasher};
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::path::Path;
use std::sync::atomic::{AtomicUsize, Ordering};

use crate::canonical::{canonical, first_difference};
use crate::package::{package_parts, rewrite_package};
use crate::writing::Writing;

/// What an operation gave, as text, or why it failed.
pub type Step = Result<String, String>;

/// Every operation on one writing of a document.
pub type Operations = dyn Fn(&[u8]) -> Outcome + Sync;

/// A save without an edit of one writing of a document.
pub type Save = dyn Fn(&[u8]) -> Result<Vec<u8>, String> + Sync;

/// The canonical form of every XML part of a package, and the size and a
/// hash of every other part, by name.
pub type Parts = BTreeMap<String, Result<String, String>>;

/// What the operations give on one writing of a document.
#[derive(Default)]
pub struct Outcome {
    pub steps: BTreeMap<&'static str, Step>,
    /// The parts each saving step wrote.
    pub saved: BTreeMap<&'static str, Parts>,
}

impl Outcome {
    pub fn step(&mut self, name: &'static str, step: Step) {
        self.steps.insert(name, step);
    }

    /// The package a step saved, read part by part.
    pub fn saved(&mut self, name: &'static str, bytes: &[u8]) {
        self.saved.insert(name, parts(bytes).unwrap_or_default());
    }
}

fn panic_text(panic: &(dyn std::any::Any + Send)) -> String {
    panic
        .downcast_ref::<String>()
        .cloned()
        .or_else(|| panic.downcast_ref::<&str>().map(|text| (*text).to_owned()))
        .unwrap_or_default()
}

/// `f`, with a panic as its message.
pub fn attempt<T>(f: impl FnOnce() -> Result<T, String>) -> Result<T, String> {
    catch_unwind(AssertUnwindSafe(f))
        .unwrap_or_else(|panic| Err(format!("panicked: {}", panic_text(&*panic))))
}

/// Every part of a package, the XML ones in their canonical form.
pub fn parts(bytes: &[u8]) -> Result<Parts, String> {
    Ok(package_parts(bytes)?
        .into_iter()
        .map(|(name, data)| {
            let read = if name.ends_with(".xml") || name.ends_with(".rels") {
                canonical(&data)
            } else {
                let mut hasher = DefaultHasher::new();
                data.hash(&mut hasher);
                Ok(format!("{} bytes, {:x}", data.len(), hasher.finish()))
            };
            (name, read)
        })
        .collect())
}

/// One document the writings are tried on.
pub struct Input {
    pub name: String,
    pub bytes: Vec<u8>,
}

/// A difference between what the operations give on a writing and on the
/// original, or a writing that could not be made.
#[derive(Clone, Debug)]
pub struct Finding {
    pub input: String,
    pub writing: Writing,
    pub operation: String,
    pub message: String,
}

/// The first line of a message, at most 300 characters.
pub fn shorten(text: &str) -> String {
    let line = text.lines().next().unwrap_or_default();
    if line.chars().count() > 300 {
        format!("{}...", line.chars().take(300).collect::<String>())
    } else {
        line.to_owned()
    }
}

/// `f` over every item on as many threads as the machine has, each with an
/// 8 MiB stack, the results in item order; a panic is `None`.
fn map_in_parallel<T: Sync, R: Send>(items: &[T], f: impl Fn(&T) -> R + Sync) -> Vec<Option<R>> {
    let threads = std::thread::available_parallelism().map_or(4, |n| n.get());
    let next = AtomicUsize::new(0);
    let mut results: Vec<(usize, Option<R>)> = std::thread::scope(|scope| {
        let workers: Vec<_> = (0..threads.min(items.len()))
            .map(|_| {
                std::thread::Builder::new()
                    .stack_size(8 * 1024 * 1024)
                    .spawn_scoped(scope, || {
                        let mut done = Vec::new();
                        loop {
                            let index = next.fetch_add(1, Ordering::Relaxed);
                            let Some(item) = items.get(index) else {
                                break done;
                            };
                            done.push((index, catch_unwind(AssertUnwindSafe(|| f(item))).ok()));
                        }
                    })
                    .expect("a worker thread starts")
            })
            .collect();
        workers
            .into_iter()
            .flat_map(|worker| worker.join().expect("a worker thread ends"))
            .collect()
    });
    results.sort_by_key(|(index, _)| *index);
    results.into_iter().map(|(_, result)| result).collect()
}

/// The parts a save without an edit changes or drops.
fn changed_by_save(
    input: &BTreeMap<String, Vec<u8>>,
    save: &Save,
    bytes: &[u8],
) -> Option<BTreeSet<String>> {
    let saved: BTreeMap<_, _> = package_parts(&attempt(|| save(bytes)).ok()?)
        .ok()?
        .into_iter()
        .collect();
    Some(
        input
            .iter()
            .filter(|(name, data)| saved.get(*name) != Some(*data))
            .map(|(name, _)| name.clone())
            .collect(),
    )
}

/// Where `rewritten` differs from `original`, operation by operation, then
/// the XML each saving step wrote, part by part.
fn differences(original: &Outcome, rewritten: &Outcome) -> Vec<(String, String)> {
    let mut found = Vec::new();
    for (operation, expected) in &original.steps {
        let message = match (expected, rewritten.steps.get(operation)) {
            // A step that did not run: an earlier one already differs.
            (_, None) => None,
            (Ok(expected), Some(Ok(got))) => first_difference(expected, got)
                .map(|at| format!("gives another result, {}", at.replace('\n', " | "))),
            (Ok(_), Some(Err(error))) => Some(error.clone()),
            (Err(error), Some(Ok(_))) => Some(format!("passes, but the original fails: {error}")),
            (Err(expected), Some(Err(error))) => {
                (expected != error).then(|| format!("fails otherwise: {error}"))
            }
        };
        if let Some(message) = message {
            found.push((operation.to_string(), shorten(&message)));
        }
    }
    for (step, expected) in &original.saved {
        let Some(got) = rewritten.saved.get(step) else {
            continue;
        };
        for (part, read) in expected {
            let message = match (read, got.get(part)) {
                (_, None) => Some("is not written".to_owned()),
                (Ok(expected), Some(Ok(got))) => first_difference(expected, got)
                    .map(|at| format!("reads otherwise, {}", at.replace('\n', " | "))),
                (Ok(_), Some(Err(error))) => Some(format!("is no XML: {error}")),
                (Err(_), _) => None,
            };
            if let Some(message) = message {
                found.push((format!("{step} xml"), shorten(&format!("{part} {message}"))));
                break;
            }
        }
    }
    found
}

/// What one run found: the differences, and the steps that fail on the
/// originals already, which no writing explains.
pub struct Run {
    pub findings: Vec<Finding>,
    pub original_failures: Vec<(String, String, String)>,
    /// How long the operations took on each original.
    pub timings: Vec<(String, std::time::Duration)>,
}

/// Each input rewritten in each writing, `run` on the original and on each
/// rewrite, and what differs. `save` saves without an edit: a part the
/// rewrite changed, which a save keeps byte for byte on the original, keeps
/// its writing too.
pub fn check(inputs: &[Input], writings: &[Writing], run: &Operations, save: &Save) -> Run {
    let originals = map_in_parallel(inputs, |input| {
        let parts: BTreeMap<_, _> = package_parts(&input.bytes)
            .expect("an input is a package")
            .into_iter()
            .collect();
        let changed = changed_by_save(&parts, save, &input.bytes);
        let started = std::time::Instant::now();
        let outcome = run(&input.bytes);
        (outcome, changed, started.elapsed())
    });
    let mut original_failures = Vec::new();
    let mut timings = Vec::new();
    for (input, original) in inputs.iter().zip(&originals) {
        let (outcome, _, elapsed) = original
            .as_ref()
            .expect("the operations run on the original");
        timings.push((input.name.clone(), *elapsed));
        for (operation, step) in &outcome.steps {
            if let Err(error) = step {
                original_failures.push((input.name.clone(), operation.to_string(), shorten(error)));
            }
        }
    }
    let cases: Vec<(usize, Writing)> = (0..inputs.len())
        .flat_map(|index| writings.iter().map(move |writing| (index, *writing)))
        .collect();
    let results = map_in_parallel(&cases, |&(index, writing)| {
        let (original, changed_on_original, _) = originals[index].as_ref().expect("checked above");
        let rewritten = match rewrite_package(&inputs[index].bytes, writing) {
            Ok(rewritten) => rewritten,
            Err(error) => return vec![("rewrite".to_owned(), shorten(&error))],
        };
        if rewritten.parts.is_empty() {
            return Vec::new();
        }
        let mut found = differences(original, &run(&rewritten.bytes));
        let rewritten_parts: BTreeMap<_, _> = package_parts(&rewritten.bytes)
            .expect("a rewrite is a package")
            .into_iter()
            .filter(|(name, _)| rewritten.parts.contains(name))
            .collect();
        if let (Some(changed), Some(on_original)) = (
            changed_by_save(&rewritten_parts, save, &rewritten.bytes),
            changed_on_original,
        ) {
            let lost: Vec<&String> = changed.difference(on_original).collect();
            if !lost.is_empty() {
                found.push((
                    "save verbatim".to_owned(),
                    format!("a save without an edit rewrites {lost:?}"),
                ));
            }
        }
        found
    });
    let findings = results
        .into_iter()
        .zip(&cases)
        .flat_map(|(found, &(index, writing))| {
            found
                .unwrap_or_else(|| vec![("harness".to_owned(), "panicked".to_owned())])
                .into_iter()
                .map(move |(operation, message)| Finding {
                    input: inputs[index].name.clone(),
                    writing,
                    operation,
                    message,
                })
        })
        .collect();
    Run {
        findings,
        original_failures,
        timings,
    }
}

/// A difference a writing gives today, from the known list. `input` is an
/// input's name, or `*` and the end of the names it stands for (`*.docx`)
/// for a class of difference met on many inputs.
pub struct Known {
    pub input: String,
    pub writing: String,
    pub operation: String,
    pub issue: u32,
    pub message: String,
    /// Met only by a build with debug assertions (`debug_assertions =
    /// true`: one of their cross-checks fails first) or only by one without
    /// (`false`: that cross-check hides it). Other builds skip the entry.
    pub debug_assertions: Option<bool>,
}

impl Known {
    fn names(&self, input: &str) -> bool {
        match self.input.strip_prefix('*') {
            Some(end) => input.ends_with(end),
            None => self.input == input,
        }
    }
}

/// Every entry of a known list (`tests/corpus/writings-known.toml`).
pub fn known(path: &Path) -> Vec<Known> {
    let text =
        std::fs::read_to_string(path).unwrap_or_else(|error| panic!("{}: {error}", path.display()));
    text.split("[[known]]")
        .skip(1)
        .map(|entry| {
            let value = |key: &str| {
                entry
                    .lines()
                    .find_map(|line| {
                        line.strip_prefix(key)
                            .and_then(|rest| rest.trim_start().strip_prefix('='))
                            .map(|v| v.trim().trim_matches('"').replace("\\\"", "\""))
                    })
                    .unwrap_or_else(|| panic!("an entry of {} has its {key}", path.display()))
            };
            Known {
                input: value("input"),
                writing: value("writing"),
                operation: value("operation"),
                issue: value("issue").parse().expect("an issue number"),
                message: value("message"),
                debug_assertions: entry.lines().find_map(|line| {
                    match line.replace(' ', "").as_str() {
                        "debug_assertions=true" => Some(true),
                        "debug_assertions=false" => Some(false),
                        _ => None,
                    }
                }),
            }
        })
        .filter(|entry| {
            entry
                .debug_assertions
                .is_none_or(|debug| debug == cfg!(debug_assertions))
        })
        .collect()
}

/// The inputs and writings `OXOFFICE_WRITINGS_ONLY` names, if it is set:
/// `OXOFFICE_WRITINGS_ONLY="word/report.docx indented"`.
pub fn only() -> Option<Vec<String>> {
    std::env::var("OXOFFICE_WRITINGS_ONLY")
        .ok()
        .map(|value| value.split_whitespace().map(str::to_owned).collect())
}

/// The writings `only` keeps: those it names, or all when it names none.
pub fn selected_writings(only: Option<&[String]>) -> Vec<Writing> {
    let named: Vec<Writing> = only
        .unwrap_or_default()
        .iter()
        .filter_map(|name| Writing::from_name(name))
        .collect();
    if named.is_empty() {
        Writing::ALL.to_vec()
    } else {
        named
    }
}

/// The inputs `only` keeps: those it names, or all when it names none.
pub fn selected_inputs(inputs: Vec<Input>, only: Option<&[String]>) -> Vec<Input> {
    let names = only.unwrap_or_default();
    if inputs.iter().any(|input| names.contains(&input.name)) {
        inputs
            .into_iter()
            .filter(|input| names.contains(&input.name))
            .collect()
    } else {
        inputs
    }
}

/// How much of the inputs and writings a run takes.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Scope {
    /// `OXOFFICE_WRITINGS_ONLY` narrowed it: no entry is stale.
    Narrowed,
    /// The pull request sample: an entry for one input is stale when its
    /// input and writing ran without it.
    Sample,
    /// Every input: an entry for many inputs (`*.docx`) is stale too when
    /// nothing matched it.
    Full,
}

/// Runs the check and fails on a difference the known list does not hold,
/// or on a stale entry (see [`Scope`]). Prints how long each original took,
/// its own failures and every finding, tab-separated, for `--nocapture`.
pub fn assert_known(
    known_list: &Path,
    inputs: &[Input],
    writings: &[Writing],
    scope: Scope,
    run: &Operations,
    save: &Save,
) {
    let result = check(inputs, writings, run, save);
    for (input, elapsed) in &result.timings {
        println!("{input}\ttime\t{:.2} s", elapsed.as_secs_f64());
    }
    for (input, operation, error) in &result.original_failures {
        println!("{input}\toriginal\t{operation}\t{error}");
    }
    let known = known(known_list);
    let mut unexpected = Vec::new();
    let mut matched = vec![false; known.len()];
    for finding in &result.findings {
        println!(
            "{}\t{}\t{}\t{}",
            finding.input,
            finding.writing.name(),
            finding.operation,
            finding.message
        );
        let same_place = |entry: &&Known| {
            entry.names(&finding.input)
                && entry.writing == finding.writing.name()
                && entry.operation == finding.operation
        };
        let what = format!(
            "{} {} {}",
            finding.input,
            finding.writing.name(),
            finding.operation
        );
        if let Some(at) = known
            .iter()
            .position(|entry| same_place(&entry) && finding.message.contains(&entry.message))
        {
            matched[at] = true;
        } else if let Some(entry) = known.iter().find(same_place) {
            unexpected.push(format!(
                "{what}: not #{} ({:?}) but {}",
                entry.issue, entry.message, finding.message
            ));
        } else {
            unexpected.push(format!("{what}: {}", finding.message));
        }
    }
    let ran: BTreeSet<(&str, &str)> = inputs
        .iter()
        .flat_map(|input| writings.iter().map(|w| (input.name.as_str(), w.name())))
        .collect();
    let stale = known.iter().zip(&matched).filter(|(entry, matched)| {
        let took = if entry.input.starts_with('*') {
            scope == Scope::Full
                && ran
                    .iter()
                    .any(|(input, writing)| entry.names(input) && *writing == entry.writing)
        } else {
            scope != Scope::Narrowed
                && ran.contains(&(entry.input.as_str(), entry.writing.as_str()))
        };
        !**matched && took
    });
    for (entry, _) in stale {
        unexpected.push(format!(
            "{} {} {} gives the same result now: remove its entry (#{}) from {}",
            entry.input,
            entry.writing,
            entry.operation,
            entry.issue,
            known_list.display()
        ));
    }
    assert!(unexpected.is_empty(), "{}", unexpected.join("\n"));
}
