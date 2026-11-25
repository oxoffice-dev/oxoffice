//! The check both facades' writings tests run: each input in each writing,
//! the operations compared with the original's, the differences matched
//! against the strict list of known ones.

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::path::Path;
use std::sync::atomic::{AtomicUsize, Ordering};

use crate::canonical::{canonical, first_difference};
use crate::writing::{Writing, package_parts, rewrite_package};

/// Every operation on one writing of a document.
pub type Operations = dyn Fn(&[u8]) -> Outcome + Sync;
/// A save without an edit.
pub type Save = dyn Fn(&[u8]) -> Result<Vec<u8>, String> + Sync;

/// What the operations give on one writing: each step's result as text or
/// its error, and the XML each saving step wrote.
#[derive(Default)]
pub struct Outcome {
    steps: BTreeMap<&'static str, Result<String, String>>,
    saved: BTreeMap<&'static str, BTreeMap<String, Result<String, String>>>,
}

impl Outcome {
    pub fn step(&mut self, name: &'static str, step: Result<String, String>) {
        self.steps.insert(name, step);
    }

    /// A step that saved: its text (or error), and the XML it wrote.
    pub fn saving(&mut self, name: &'static str, result: &Result<(Vec<u8>, String), String>) {
        let (bytes, text) = match result {
            Ok((bytes, text)) => (Some(bytes), Ok(text.clone())),
            Err(error) => (None, Err(error.clone())),
        };
        self.step(name, text);
        bytes.into_iter().for_each(|bytes| self.saved(name, bytes));
    }

    /// The XML parts of the package a step saved, in their canonical form.
    fn saved(&mut self, name: &'static str, bytes: &[u8]) {
        let parts = package_parts(bytes).unwrap_or_default().into_iter();
        let xml = parts.filter(|(part, _)| part.ends_with(".xml") || part.ends_with(".rels"));
        self.saved.insert(
            name,
            xml.map(|(part, data)| (part, canonical(&data))).collect(),
        );
    }
}

/// `f`, with a panic as its message.
pub fn attempt<T>(f: impl FnOnce() -> Result<T, String>) -> Result<T, String> {
    catch_unwind(AssertUnwindSafe(f)).unwrap_or_else(|panic| {
        let text = panic.downcast_ref::<String>().cloned();
        let text = text.or_else(|| panic.downcast_ref::<&str>().map(|text| (*text).to_owned()));
        Err(format!("panicked: {}", text.unwrap_or_default()))
    })
}

/// One document the writings are tried on: its name and its bytes.
pub type Input = (String, Vec<u8>);

fn shorten(text: &str) -> String {
    text.lines()
        .next()
        .unwrap_or_default()
        .chars()
        .take(300)
        .collect()
}

/// `f` over every item on as many threads as the machine has, with 8 MiB
/// stacks, the results in item order; a panic is `None`.
fn in_parallel<T: Sync, R: Send>(items: &[T], f: impl Fn(&T) -> R + Sync) -> Vec<Option<R>> {
    let next = AtomicUsize::new(0);
    let threads = std::thread::available_parallelism().map_or(4, |n| n.get());
    let mut results: Vec<(usize, Option<R>)> = std::thread::scope(|scope| {
        let work = || {
            let mut done = Vec::new();
            loop {
                let index = next.fetch_add(1, Ordering::Relaxed);
                let Some(item) = items.get(index) else {
                    break done;
                };
                done.push((index, catch_unwind(AssertUnwindSafe(|| f(item))).ok()));
            }
        };
        let workers: Vec<_> = (0..threads.min(items.len()))
            .map(|_| {
                std::thread::Builder::new()
                    .stack_size(8 << 20)
                    .spawn_scoped(scope, work)
            })
            .collect::<Result<_, _>>()
            .expect("worker threads start");
        workers
            .into_iter()
            .flat_map(|worker| worker.join().expect("a worker ends"))
            .collect()
    });
    results.sort_by_key(|(index, _)| *index);
    results.into_iter().map(|(_, result)| result).collect()
}

/// The parts a save without an edit changes or drops, of those given.
fn changed_by_save(
    parts: &[(String, Vec<u8>)],
    save: &Save,
    bytes: &[u8],
) -> Option<BTreeSet<String>> {
    let saved: HashMap<_, _> = package_parts(&attempt(|| save(bytes)).ok()?)
        .ok()?
        .into_iter()
        .collect();
    Some(
        parts
            .iter()
            .filter(|(name, data)| saved.get(name) != Some(data))
            .map(|(name, _)| name.clone())
            .collect(),
    )
}

/// Where `rewritten` differs from `original`: step by step, then the XML each
/// saving step wrote, part by part.
fn differences(original: &Outcome, rewritten: &Outcome) -> Vec<(String, String)> {
    let mut found = Vec::new();
    for (step, expected) in &original.steps {
        let message = match (expected, rewritten.steps.get(step)) {
            (_, None) => None, // an earlier step already differs
            (Ok(expected), Some(Ok(got))) => {
                first_difference(expected, got).map(|at| format!("gives another result, {at}"))
            }
            (Ok(_), Some(Err(error))) => Some(error.clone()),
            (Err(error), Some(Ok(_))) => Some(format!("passes, but the original fails: {error}")),
            (Err(expected), Some(Err(error))) => {
                (expected != error).then(|| format!("fails otherwise: {error}"))
            }
        };
        found.extend(message.map(|message| (step.to_string(), shorten(&message))));
    }
    for (step, expected) in &original.saved {
        let Some(got) = rewritten.saved.get(step) else {
            continue;
        };
        // Every part either save wrote.
        let parts: BTreeSet<&String> = expected.keys().chain(got.keys()).collect();
        let differs =
            parts
                .into_iter()
                .find_map(|part| match (expected.get(part), got.get(part)) {
                    (Some(expected), Some(got)) if expected == got => None,
                    (Some(Ok(expected)), Some(Ok(got))) => first_difference(expected, got)
                        .map(|at| format!("{part} reads otherwise, {at}")),
                    (_, None) => Some(format!("{part} is not written")),
                    (None, _) => Some(format!("{part} is written by this writing only")),
                    (_, Some(Err(error))) => Some(format!("{part} is no XML: {error}")),
                    (Some(Err(error)), _) => Some(format!("{part} was no XML: {error}")),
                });
        found.extend(differs.map(|message| (format!("{step} xml"), shorten(&message))));
    }
    found
}

/// The entries of the known list, each `key = value` line of one
/// `[[known]]` table by key, those for the other kind of build left out
/// (`debug_assertions = true` or `false`).
fn read_known(path: &Path) -> Vec<HashMap<String, String>> {
    let text = std::fs::read_to_string(path).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
    let entries = text.split("[[known]]").skip(1).map(|entry| {
        let pairs = entry.lines().filter_map(|line| line.split_once(" = "));
        let unquoted = |value: &str| value.trim_matches('"').replace("\\\"", "\"");
        pairs
            .map(|(key, value)| (key.to_owned(), unquoted(value)))
            .collect::<HashMap<_, _>>()
    });
    let debug = cfg!(debug_assertions).to_string();
    entries
        .filter(|entry| {
            entry
                .get("debug_assertions")
                .is_none_or(|value| *value == debug)
        })
        .collect()
}

/// Whether an entry's `input` names this input: one name, or `*.docx`.
fn names(entry: &HashMap<String, String>, input: &str) -> bool {
    entry["input"]
        .strip_prefix('*')
        .map_or(entry["input"] == input, |end| input.ends_with(end))
}

/// Runs `run` on each input and on each of its writings, and fails on a
/// difference `known_list` does not hold, on one of its entries that did not
/// happen (one for a kind of inputs only when `full`), or on a writing that
/// changed no input. Prints the originals' own failures and every
/// difference, tab-separated, for `--nocapture`.
pub fn check_writings(known: &Path, inputs: Vec<Input>, full: bool, run: &Operations, save: &Save) {
    let originals = in_parallel(&inputs, |(_, bytes)| {
        let parts = package_parts(bytes).expect("an input is a package");
        (run(bytes), changed_by_save(&parts, save, bytes))
    });
    for ((name, _), original) in inputs.iter().zip(&originals) {
        for (step, result) in &original.as_ref().expect("the original runs").0.steps {
            if let Err(error) = result {
                println!("{name}\toriginal\t{step}\t{}", shorten(error));
            }
        }
    }
    let cases: Vec<(usize, Writing)> = (0..inputs.len())
        .flat_map(|index| Writing::ALL.map(|writing| (index, writing)))
        .collect();
    let results = in_parallel(&cases, |&(index, writing)| {
        let (original, on_original) = originals[index].as_ref().expect("checked above");
        let (bytes, changed) = match rewrite_package(&inputs[index].1, writing) {
            Ok(rewritten) => rewritten,
            Err(error) => return (true, vec![("rewrite".to_owned(), shorten(&error))]),
        };
        if changed.is_empty() {
            return (false, Vec::new());
        }
        let mut found = differences(original, &run(&bytes));
        // A save keeps byte for byte every part it keeps on the original.
        let parts = package_parts(&bytes).expect("a package").into_iter();
        let parts: Vec<_> = parts.filter(|(name, _)| changed.contains(name)).collect();
        if let (Some(lost), Some(kept)) = (changed_by_save(&parts, save, &bytes), on_original)
            && lost.difference(kept).next().is_some()
        {
            let lost: Vec<_> = lost.difference(kept).collect();
            found.push((
                "save verbatim".into(),
                format!("a save without an edit rewrites {lost:?}"),
            ));
        }
        (true, found)
    });
    let known = read_known(known);
    let (mut unexpected, mut matched, mut changing) =
        (Vec::new(), vec![false; known.len()], BTreeSet::new());
    for (&(index, writing), result) in cases.iter().zip(results) {
        let (changed, found) =
            result.unwrap_or((true, vec![("harness".into(), "panicked".into())]));
        if changed {
            changing.insert(writing.name());
        }
        let input = &inputs[index].0;
        for (operation, message) in found {
            println!("{input}\t{}\t{operation}\t{message}", writing.name());
            let place = |entry: &&HashMap<String, String>| {
                names(entry, input)
                    && entry["writing"] == writing.name()
                    && entry["operation"] == operation
            };
            let what = format!("{input} {} {operation}", writing.name());
            match known
                .iter()
                .position(|entry| place(&entry) && message.contains(&entry["message"]))
            {
                Some(at) => matched[at] = true,
                None => match known.iter().find(place) {
                    Some(entry) => {
                        unexpected.push(format!("{what}: not #{} but {message}", entry["issue"]))
                    }
                    None => unexpected.push(format!("{what}: {message}")),
                },
            }
        }
    }
    for (entry, _) in known.iter().zip(&matched).filter(|(_, matched)| !**matched) {
        let ran = inputs.iter().any(|(name, _)| names(entry, name));
        // An entry of this kind of input that the full run never takes.
        let kind = |name: &str| name.rsplit('.').next().unwrap_or_default().to_owned();
        let ours = inputs
            .iter()
            .any(|(name, _)| kind(name) == kind(&entry["input"]));
        if (ran && (full || !entry["input"].starts_with('*')))
            || (full && ours && !ran)
            || !Writing::ALL.iter().any(|w| w.name() == entry["writing"])
        {
            unexpected.push(format!(
                "{} {} {}: does not happen, remove its entry (#{})",
                entry["input"], entry["writing"], entry["operation"], entry["issue"]
            ));
        }
    }
    for writing in Writing::ALL
        .iter()
        .filter(|writing| !changing.contains(&writing.name()))
    {
        unexpected.push(format!("{} rewrites no input", writing.name()));
    }
    assert!(unexpected.is_empty(), "{}", unexpected.join("\n"));
}
