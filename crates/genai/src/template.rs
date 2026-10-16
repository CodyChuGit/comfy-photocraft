//! Workflow templates: API-format ComfyUI graphs with `{{placeholder}}` values, plus the metadata
//! the engine and the picker need (task, licence class, model files, defaults).
//!
//! A template file is one JSON document `{"meta": {...}, "graph": {...}}`. The graph is exactly
//! what ComfyUI's **Workflow › Export (API)** writes, with the values PhotoCraft supplies replaced
//! by `"{{name}}"` strings. Filling is typed: `meta.placeholders` says whether a binding becomes
//! a JSON string, integer or float, so `"{{steps}}"` turns into `40`, not `"40"`.
//!
//! The built-in templates are compiled in ([`builtin`]); users can import their own later.

use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::{Error, Result, Task};

/// Licence class of the model a template drives, shown in the picker and gated by preferences.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum License {
    /// Apache-2.0 / MIT: usable by anyone for anything.
    Permissive,
    /// Open weights with conditions (revenue caps, moderation duties).
    Community,
    /// Non-commercial / research-only.
    Research,
}

/// JSON type a placeholder binding is coerced to.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Kind {
    String,
    Int,
    Float,
}

impl Kind {
    fn coerce(self, name: &str, v: &Value) -> Result<Value> {
        let bad = || Error::Template(format!("placeholder `{name}` expects a {self:?}, got {v}"));
        match self {
            Kind::String => match v {
                Value::String(_) => Ok(v.clone()),
                Value::Number(n) => Ok(Value::String(n.to_string())),
                Value::Bool(b) => Ok(Value::String(b.to_string())),
                _ => Err(bad()),
            },
            Kind::Int => {
                let n = v.as_f64().ok_or_else(bad)?;
                if !n.is_finite() || n.fract() != 0.0 || n < 0.0 || n >= 9_007_199_254_740_992.0 {
                    return Err(bad());
                }
                Ok(Value::from(n as u64))
            }
            Kind::Float => {
                let n = v.as_f64().ok_or_else(bad)?;
                if !n.is_finite() {
                    return Err(bad());
                }
                serde_json::Number::from_f64(n).map(Value::Number).ok_or_else(bad)
            }
        }
    }
}

/// A model file the graph loads, and where ComfyUI keeps it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ModelSlot {
    /// The placeholder whose value is the file name (`unet`, `clip`, `vae`, `lora`…).
    pub placeholder: String,
    /// The `ComfyUI/models/<folder>` the file lives in (also the `/models/<folder>` route).
    pub folder: String,
    pub default: String,
    /// The same weights in NVFP4 (Comfy-Org's official repack), which Blackwell GPUs run through
    /// their 4-bit tensor cores: smaller and a little faster, the output alike. A backend whose
    /// precision policy allows it takes this file instead of `default` when the server has it
    /// and the caller bound nothing for the slot.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub nvfp4: Option<String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct Defaults {
    pub steps: u32,
    pub guidance: f32,
}

impl Default for Defaults {
    fn default() -> Self {
        Self { steps: 20, guidance: 1.0 }
    }
}

/// Reference images a template takes besides its main image (PhotoshopEX's reference deck).
/// Each placeholder is a `LoadImage` file name; `nodes` lists, per placeholder, the graph nodes
/// that exist only for it. A reference the caller does not give is pruned: its nodes go, and so
/// does every input of another node that links to them (the inputs must be optional ones, such
/// as `TextEncodeQwenImageEditPlus`'s `image2`).
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct References {
    /// At most this many (0 = the template takes none).
    pub max: u32,
    /// At least this many (a reference-driven template).
    pub min: u32,
    /// The placeholders in the order references bind to them.
    pub placeholders: Vec<String>,
    pub nodes: BTreeMap<String, Vec<String>>,
}

/// Output sizes a text-to-image template offers (empty = not an image template).
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct Sizes {
    /// `"1:1"`, `"3:2"`, … in the order the picker shows them.
    pub aspects: Vec<String>,
    /// Shorter and longer side limits (pixels) and the grid both sides snap to.
    pub min: u32,
    pub max: u32,
    pub step: u32,
}

/// How many variations one request may ask for.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct Counts {
    pub max: u32,
    pub default: u32,
}

impl Default for Counts {
    fn default() -> Self {
        Self { max: 4, default: 1 }
    }
}

/// A run's expected duration on a warm server: `base + perMegapixel × megapixels +
/// perReference × extra reference images` seconds per variation (measured on the reference
/// machine, see `docs/comfy/benchmarks.md`). A reference-driven text-to-image template's first
/// reference stands in for the picture and is in `base`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct Estimate {
    pub base: f32,
    pub per_megapixel: f32,
    pub per_reference: f32,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Meta {
    /// `family/task`, e.g. `qwen-edit-2511/fill`. Lower-case letters, digits, `-`, `.`, `/`.
    pub id: String,
    pub name: String,
    pub family: String,
    pub task: Task,
    pub license: License,
    #[serde(default)]
    pub license_note: String,
    /// Oldest ComfyUI that has every node (`major.minor.patch`); empty = unknown.
    #[serde(default)]
    pub min_comfy_version: String,
    /// The node whose `images` output is the result.
    pub save_node: String,
    #[serde(default)]
    pub needs_image: bool,
    #[serde(default)]
    pub needs_mask: bool,
    #[serde(default)]
    pub defaults: Defaults,
    #[serde(default)]
    pub placeholders: BTreeMap<String, Kind>,
    #[serde(default)]
    pub models: Vec<ModelSlot>,
    /// How the user's prompt is wrapped before it reaches the model: `{prompt}` is replaced by
    /// the text (instruction-following editors want "change X to …", not a bare noun phrase).
    /// Empty = the prompt as typed.
    #[serde(default)]
    pub prompt_format: String,
    /// The wrapper used instead when the prompt already is an instruction (it starts with an
    /// imperative verb such as "remove" or "replace", see [`IMPERATIVE_OPENERS`]); empty = use
    /// `prompt_format` for those too.
    #[serde(default)]
    pub prompt_format_imperative: String,
    /// For text-to-image templates whose model can output transparency: how the prompt asks
    /// for it (`{prompt}` replaced), used when the caller wants an RGBA result. Empty = the
    /// model cannot, and a request for transparency is refused.
    #[serde(default)]
    pub prompt_format_transparent: String,
    #[serde(default)]
    pub notes: String,
    /// Who made the model ("Alibaba Qwen", "Krea", "Meta"), for the picker.
    #[serde(default)]
    pub creator: String,
    /// One line for the picker ("the fast local edit model").
    #[serde(default)]
    pub description: String,
    /// Free-text badges for the picker (it adds the time estimate itself).
    #[serde(default)]
    pub badges: Vec<String>,
    #[serde(default)]
    pub references: References,
    #[serde(default)]
    pub sizes: Sizes,
    #[serde(default)]
    pub counts: Counts,
    #[serde(default)]
    pub estimated_seconds: Estimate,
    /// The Generate panel pages that offer this template: `image`, `fill`, `edit`, `expand`,
    /// `matte`, `split`, `select`, `enhance`.
    #[serde(default)]
    pub pages: Vec<String>,
}

impl Meta {
    /// Expected seconds for `count` variations at `width × height` with `references` reference
    /// images on a warm server (0 when the template has no estimate). One formula for the UI,
    /// the CLI and scripts.
    pub fn estimate_seconds(&self, width: u32, height: u32, count: u32, references: u32) -> f32 {
        let e = &self.estimated_seconds;
        let mp = f64::from(width) * f64::from(height) / 1_000_000.0;
        let free = u32::from(!self.needs_image && self.references.min > 0);
        let extra = f64::from(references.saturating_sub(free));
        let one = f64::from(e.base) + f64::from(e.per_megapixel) * mp + f64::from(e.per_reference) * extra;
        (one * f64::from(count.max(1))).clamp(0.0, 1.0e6) as f32
    }
}

/// A parsed template.
#[derive(Clone, Debug, PartialEq)]
pub struct Template {
    pub meta: Meta,
    pub graph: Value,
}

#[derive(Deserialize)]
struct File {
    meta: Meta,
    graph: Value,
}

const BUILTIN: &[&str] = &[
    include_str!("../workflows/qwen-edit-2511-fill.json"),
    include_str!("../workflows/qwen-edit-2511-fill-lightning-8.json"),
    include_str!("../workflows/qwen-edit-2511-fill-lightning-4.json"),
    include_str!("../workflows/qwen-edit-2511-fill-guided.json"),
    include_str!("../workflows/qwen-edit-2511-expand-lightning-8.json"),
    include_str!("../workflows/qwen-edit-2511-expand.json"),
    include_str!("../workflows/qwen-edit-2511-edit-lightning-8.json"),
    include_str!("../workflows/qwen-edit-2511-edit.json"),
    include_str!("../workflows/qwen-2.1-fill.json"),
    include_str!("../workflows/krea2-turbo-image.json"),
    include_str!("../workflows/qwen-2.1-image.json"),
    include_str!("../workflows/qwen-2.1-matte.json"),
    include_str!("../workflows/sam3.1-segment.json"),
    include_str!("../workflows/sam3.1-segment-point.json"),
    include_str!("../workflows/qwen-layered-split.json"),
    include_str!("../workflows/enhance-edit.json"),
    include_str!("../workflows/enhance-image.json"),
    include_str!("../workflows/qwen-edit-2511-image-ref.json"),
    include_str!("../workflows/krea2-turbo-image-style.json"),
    include_str!("../workflows/qwen-2.1-edit.json"),
    include_str!("../workflows/qwen-edit-2511-angles.json"),
    include_str!("../workflows/sam3.1-segment-box.json"),
];

/// Verbs a prompt can open with when it is already an edit instruction ("remove the car",
/// "make the sky stormy") rather than a description of new content ("a red boat").
pub const IMPERATIVE_OPENERS: &[&str] = &[
    "add",
    "remove",
    "delete",
    "erase",
    "replace",
    "change",
    "make",
    "turn",
    "put",
    "fill",
    "paint",
    "convert",
    "swap",
    "move",
    "give",
    "extend",
    "clean",
    "restore",
    "fix",
    "repair",
    "cover",
    "color",
    "colour",
    "recolor",
    "recolour",
    "blur",
    "sharpen",
    "brighten",
    "darken",
    "transform",
    "draw",
    "place",
    "insert",
    "render",
    "create",
    "generate",
    "show",
    "hide",
    "open",
    "close",
    "rotate",
    "flip",
    "crop",
    "redraw",
    "repaint",
    "retouch",
    "smooth",
    "straighten",
    "upscale",
    "enhance",
    "edit",
    "modify",
    "adjust",
    "apply",
    "let",
    "keep",
    "set",
    "write",
];

/// Does the prompt start with one of [`IMPERATIVE_OPENERS`] (case-insensitive, first word)?
pub fn is_imperative(prompt: &str) -> bool {
    let first = prompt.trim().split(|c: char| !c.is_alphanumeric()).next().unwrap_or("").to_ascii_lowercase();
    !first.is_empty() && IMPERATIVE_OPENERS.contains(&first.as_str())
}

/// Every compiled-in template. Parsing cannot fail for shipped files (a test checks it); a file
/// that fails anyway is skipped rather than taking the others down.
pub fn builtin() -> Vec<Template> {
    BUILTIN.iter().filter_map(|src| Template::parse(src).ok()).collect()
}

/// The built-in template with this id.
pub fn find(id: &str) -> Result<Template> {
    builtin().into_iter().find(|t| t.meta.id == id).ok_or_else(|| {
        let known: Vec<String> = builtin().into_iter().map(|t| t.meta.id).collect();
        Error::Template(format!("unknown template `{id}`; available: {}", known.join(", ")))
    })
}

fn is_placeholder(v: &Value) -> Option<&str> {
    v.as_str().and_then(|s| s.strip_prefix("{{")).and_then(|s| s.strip_suffix("}}")).map(str::trim).filter(|s| !s.is_empty())
}

fn walk<'a>(v: &'a Value, out: &mut Vec<&'a str>) {
    match v {
        Value::Object(m) => m.values().for_each(|x| walk(x, out)),
        Value::Array(a) => a.iter().for_each(|x| walk(x, out)),
        other => {
            if let Some(p) = is_placeholder(other) {
                out.push(p);
            }
        }
    }
}

fn substitute(v: &Value, bindings: &BTreeMap<String, Value>, kinds: &BTreeMap<String, Kind>) -> Result<Value> {
    Ok(match v {
        Value::Object(m) => {
            let mut out = serde_json::Map::with_capacity(m.len());
            for (k, x) in m {
                out.insert(k.clone(), substitute(x, bindings, kinds)?);
            }
            Value::Object(out)
        }
        Value::Array(a) => Value::Array(a.iter().map(|x| substitute(x, bindings, kinds)).collect::<Result<Vec<_>>>()?),
        other => match is_placeholder(other) {
            Some(name) => {
                let b = bindings.get(name).ok_or_else(|| Error::Template(format!("placeholder `{name}` is not bound")))?;
                kinds.get(name).copied().unwrap_or(Kind::String).coerce(name, b)?
            }
            None => other.clone(),
        },
    })
}

impl Template {
    /// Parse one template file (`{"meta": …, "graph": …}`), checking its shape.
    pub fn parse(src: &str) -> Result<Template> {
        let f: File = serde_json::from_str(src).map_err(|e| Error::Template(format!("not a template file: {e}")))?;
        let t = Template { meta: f.meta, graph: f.graph };
        t.check()?;
        Ok(t)
    }

    fn check(&self) -> Result<()> {
        let m = &self.meta;
        if m.id.is_empty() || !m.id.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || matches!(c, '-' | '.' | '/' | '_')) {
            return Err(Error::Template(format!("bad template id `{}`", m.id)));
        }
        let nodes = self.graph.as_object().ok_or_else(|| Error::Template(format!("{}: graph must be an object of nodes", m.id)))?;
        if nodes.is_empty() {
            return Err(Error::Template(format!("{}: graph has no nodes", m.id)));
        }
        for (id, n) in nodes {
            if n.get("class_type").and_then(Value::as_str).is_none() || !n.get("inputs").is_some_and(Value::is_object) {
                return Err(Error::Template(format!("{}: node {id} needs `class_type` and `inputs` (is this the UI format, not the API format?)", m.id)));
            }
        }
        if !nodes.contains_key(&m.save_node) {
            return Err(Error::Template(format!("{}: saveNode {} is not in the graph", m.id, m.save_node)));
        }
        for p in self.placeholders() {
            if !m.placeholders.contains_key(p) {
                return Err(Error::Template(format!("{}: placeholder `{p}` has no type in meta.placeholders", m.id)));
            }
        }
        for slot in &m.models {
            if !m.placeholders.contains_key(&slot.placeholder) {
                return Err(Error::Template(format!("{}: model slot `{}` is not a placeholder", m.id, slot.placeholder)));
            }
        }
        let r = &m.references;
        if r.max as usize > r.placeholders.len() || r.min > r.max {
            return Err(Error::Template(format!("{}: references need min ≤ max ≤ the placeholders listed", m.id)));
        }
        for p in &r.placeholders {
            if !m.placeholders.contains_key(p) {
                return Err(Error::Template(format!("{}: reference `{p}` is not a placeholder", m.id)));
            }
            for n in r.nodes.get(p).map(Vec::as_slice).unwrap_or_default() {
                if !nodes.contains_key(n) {
                    return Err(Error::Template(format!("{}: reference `{p}` names node {n}, which is not in the graph", m.id)));
                }
            }
        }
        Ok(())
    }

    /// The graph without the nodes of references `bindings` leaves unbound, and without the
    /// inputs that linked to them (see [`References`]).
    fn graph_for(&self, bindings: &BTreeMap<String, Value>) -> Value {
        let r = &self.meta.references;
        let gone: BTreeSet<&str> = r
            .placeholders
            .iter()
            .filter(|p| !bindings.contains_key(p.as_str()))
            .flat_map(|p| r.nodes.get(p).into_iter().flatten())
            .map(String::as_str)
            .collect();
        let mut graph = self.graph.clone();
        if gone.is_empty() {
            return graph;
        }
        if let Some(nodes) = graph.as_object_mut() {
            nodes.retain(|id, _| !gone.contains(id.as_str()));
            for n in nodes.values_mut() {
                if let Some(inputs) = n.get_mut("inputs").and_then(Value::as_object_mut) {
                    inputs.retain(|_, v| !v.as_array().is_some_and(|a| a.len() == 2 && a.first().and_then(Value::as_str).is_some_and(|id| gone.contains(id))));
                }
            }
        }
        graph
    }

    /// The prompt as the model should see it: `meta.promptFormatImperative` when the prompt
    /// already is an instruction (see [`is_imperative`]) and the template has one, else
    /// `meta.promptFormat`; an empty format passes the prompt through.
    pub fn format_prompt(&self, prompt: &str) -> String {
        let prompt = prompt.trim();
        let imperative = self.meta.prompt_format_imperative.trim();
        let f = if is_imperative(prompt) && !imperative.is_empty() { imperative } else { self.meta.prompt_format.trim() };
        if f.is_empty() {
            prompt.to_string()
        } else if f.contains("{prompt}") {
            f.replace("{prompt}", prompt)
        } else {
            format!("{f} {prompt}")
        }
    }

    /// Placeholder names used in the graph, each once, in first-seen order.
    pub fn placeholders(&self) -> Vec<&str> {
        let mut all = Vec::new();
        walk(&self.graph, &mut all);
        let mut seen = BTreeSet::new();
        all.into_iter().filter(|p| seen.insert(*p)).collect()
    }

    /// Every `class_type` the graph needs the server to have.
    pub fn class_types(&self) -> BTreeSet<String> {
        self.graph.as_object().map(|m| m.values().filter_map(|n| n.get("class_type").and_then(Value::as_str)).map(str::to_string).collect()).unwrap_or_default()
    }

    /// The graph with every placeholder replaced by its typed binding. Unbound placeholders are
    /// an error, except reference placeholders, whose nodes are pruned (see [`References`]);
    /// extra bindings are ignored.
    pub fn fill(&self, bindings: &BTreeMap<String, Value>) -> Result<Value> {
        substitute(&self.graph_for(bindings), bindings, &self.meta.placeholders)
    }

    /// Bindings for the model slots: the template defaults, overridden by `overrides`
    /// (`placeholder → file name`). Unknown override names are an error so typos surface.
    pub fn model_bindings(&self, overrides: &[(String, String)]) -> Result<BTreeMap<String, Value>> {
        let mut out: BTreeMap<String, Value> = self.meta.models.iter().map(|s| (s.placeholder.clone(), Value::String(s.default.clone()))).collect();
        for (k, v) in overrides {
            if !out.contains_key(k) {
                let known: Vec<&str> = self.meta.models.iter().map(|s| s.placeholder.as_str()).collect();
                return Err(Error::Request(format!("template `{}` has no model slot `{k}` (slots: {})", self.meta.id, known.join(", "))));
            }
            if v.is_empty() || v.contains(['/', '\\', '\0']) || v.contains("..") {
                return Err(Error::Request(format!("`{k}` must be a plain model file name, got `{v}`")));
            }
            out.insert(k.clone(), Value::String(v.clone()));
        }
        Ok(out)
    }

    /// Nodes the server does not know, from a `GET /object_info` answer (an object keyed by
    /// class name). Empty means the graph can run.
    pub fn missing_nodes(&self, object_info: &Value) -> Vec<String> {
        let known = object_info.as_object();
        self.class_types().into_iter().filter(|c| !known.is_some_and(|m| m.contains_key(c))).collect()
    }
}

/// `a` is at least `b`, comparing dotted numeric versions (`0.37.0` ≥ `0.26`); unparsable parts
/// count as zero so an odd version string never blocks a run.
pub fn version_at_least(a: &str, b: &str) -> bool {
    fn parts(s: &str) -> Vec<u64> {
        s.trim().trim_start_matches('v').split('.').map(|p| p.chars().take_while(char::is_ascii_digit).collect::<String>().parse().unwrap_or(0)).collect()
    }
    let (a, b) = (parts(a), parts(b));
    for i in 0..a.len().max(b.len()) {
        let (x, y) = (a.get(i).copied().unwrap_or(0), b.get(i).copied().unwrap_or(0));
        if x != y {
            return x > y;
        }
    }
    true
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn prompts_are_wrapped_as_descriptions_or_as_instructions() {
        assert!(is_imperative("Remove the person on the left") && is_imperative("replace it with grass") && is_imperative("  make it night"));
        assert!(!is_imperative("a small red boat") && !is_imperative("") && !is_imperative("Adding machines"));
        let t = find("qwen-edit-2511/fill-lightning-8").unwrap_or_else(|e| panic!("{e}"));
        let noun = t.format_prompt(" a small red boat ");
        assert!(noun.starts_with("Add a small red boat to this image"), "{noun}");
        let verb = t.format_prompt("remove the person on the left");
        assert!(verb.starts_with("remove the person on the left. Fit the result"), "{verb}");
        // The 2.1 template keeps its mask instruction either way.
        let q = find("qwen-2.1/fill").unwrap_or_else(|e| panic!("{e}"));
        assert!(q.format_prompt("replace the boat with a buoy").contains("as follows: replace the boat with a buoy"));
        assert!(q.format_prompt("a buoy").contains("so that it shows a buoy"));
        // A template without formats passes the prompt through.
        let s = find("sam3.1/segment").unwrap_or_else(|e| panic!("{e}"));
        assert_eq!(s.format_prompt("the dog"), "the dog");
    }

    #[test]
    fn every_builtin_template_parses() {
        assert_eq!(builtin().len(), BUILTIN.len(), "a built-in template failed to parse");
        for t in builtin() {
            assert!(t.graph.get(&t.meta.save_node).is_some(), "{}", t.meta.id);
            assert!(!t.placeholders().is_empty(), "{}", t.meta.id);
        }
    }

    #[test]
    fn fill_substitutes_typed_values_and_reports_unbound() {
        let t = find("qwen-edit-2511/fill").unwrap_or_else(|e| panic!("{e}"));
        let mut b = t.model_bindings(&[]).unwrap_or_else(|e| panic!("{e}"));
        for (k, v) in [
            ("prompt", json!("a red bicycle")),
            ("negative", json!("")),
            ("seed", json!(7.0)),
            ("steps", json!(4)),
            ("cfg", json!(1)),
            ("image", json!("in.png")),
            ("mask", json!("m.png")),
        ] {
            b.insert(k.into(), v);
        }
        assert!(matches!(t.fill(&b), Err(Error::Template(m)) if m.contains("prefix")));
        b.insert("prefix".into(), json!("photocraft/x"));
        let g = t.fill(&b).unwrap_or_else(|e| panic!("{e}"));
        assert_eq!(g["14"]["inputs"]["seed"], json!(7));
        assert_eq!(g["14"]["inputs"]["steps"], json!(4));
        assert_eq!(g["14"]["inputs"]["cfg"], json!(1.0));
        assert_eq!(g["6"]["inputs"]["prompt"], json!("a red bicycle"));
        assert_eq!(g["1"]["inputs"]["unet_name"], json!("qwen_image_edit_2511_fp8mixed.safetensors"));
        assert_eq!(g["6"]["inputs"]["clip"], json!(["2", 0]), "links are untouched");
        assert!(g.to_string().find("{{").is_none(), "no placeholder left");
    }

    #[test]
    fn prompt_formats_wrap_the_prompt() {
        let plain = find("sam3.1/segment").unwrap_or_else(|e| panic!("{e}"));
        assert_eq!(plain.format_prompt("a red bicycle"), "a red bicycle");
        let edit = find("qwen-edit-2511/fill").unwrap_or_else(|e| panic!("{e}"));
        assert!(edit.format_prompt("a red bicycle").starts_with("Add a red bicycle to this image"));
        let wrapped = find("qwen-2.1/fill").unwrap_or_else(|e| panic!("{e}"));
        let p = wrapped.format_prompt("a red bicycle");
        assert!(p.contains("a red bicycle") && p.contains("image 2") && !p.contains("{prompt}"), "{p}");
        let mut t = plain.clone();
        t.meta.prompt_format = "Make it:".into();
        assert_eq!(t.format_prompt("blue"), "Make it: blue");
    }

    #[test]
    fn model_overrides_are_checked() {
        let t = find("qwen-edit-2511/fill").unwrap_or_else(|e| panic!("{e}"));
        let b = t.model_bindings(&[("unet".into(), "qwen_image_edit_2511_bf16.safetensors".into())]).unwrap_or_else(|e| panic!("{e}"));
        assert_eq!(b["unet"], json!("qwen_image_edit_2511_bf16.safetensors"));
        assert!(t.model_bindings(&[("lora".into(), "x".into())]).is_err());
        assert!(t.model_bindings(&[("unet".into(), "../x".into())]).is_err());
        assert!(t.model_bindings(&[("unet".into(), String::new())]).is_err());
    }

    #[test]
    fn bad_bindings_are_errors() {
        let t = find("qwen-edit-2511/fill").unwrap_or_else(|e| panic!("{e}"));
        let mut b = t.model_bindings(&[]).unwrap_or_else(|e| panic!("{e}"));
        for k in ["prompt", "negative", "image", "mask", "prefix"] {
            b.insert(k.into(), json!("x"));
        }
        b.insert("cfg".into(), json!(1.0));
        b.insert("steps".into(), json!(4));
        b.insert("seed".into(), json!(-1));
        assert!(t.fill(&b).is_err(), "negative seed");
        b.insert("seed".into(), json!(1.5));
        assert!(t.fill(&b).is_err(), "fractional int");
        b.insert("seed".into(), json!("7"));
        assert!(t.fill(&b).is_err(), "string for int");
    }

    #[test]
    fn missing_nodes_and_versions() {
        let t = find("qwen-edit-2511/fill").unwrap_or_else(|e| panic!("{e}"));
        let mut info = serde_json::Map::new();
        for c in t.class_types() {
            info.insert(c, json!({}));
        }
        assert!(t.missing_nodes(&Value::Object(info.clone())).is_empty());
        info.remove("TextEncodeQwenImageEditPlus");
        assert_eq!(t.missing_nodes(&Value::Object(info)), vec!["TextEncodeQwenImageEditPlus".to_string()]);
        assert!(version_at_least("0.37.0", "0.26"));
        assert!(version_at_least("v0.37.0", "0.37.0"));
        assert!(!version_at_least("0.25.9", "0.26.0"));
        assert!(version_at_least("garbage", ""));
    }

    fn bound(t: &Template, extra: &[(&str, Value)]) -> BTreeMap<String, Value> {
        let mut b = t.model_bindings(&[]).unwrap_or_else(|e| panic!("{e}"));
        for (k, v) in [("prompt", json!("p")), ("negative", json!("")), ("seed", json!(1)), ("steps", json!(8)), ("cfg", json!(1.0)), ("prefix", json!("x"))] {
            b.insert(k.into(), v);
        }
        for (k, v) in extra {
            b.insert((*k).to_string(), v.clone());
        }
        b
    }

    #[test]
    fn unbound_references_are_pruned_with_the_inputs_that_used_them() {
        let t = find("qwen-edit-2511/edit-lightning-8").unwrap_or_else(|e| panic!("{e}"));
        assert_eq!(t.meta.references.max, 2);
        // No references: the two LoadImage nodes and the encoders' image2/image3 inputs go.
        let g = t.fill(&bound(&t, &[("image", json!("in.png"))])).unwrap_or_else(|e| panic!("{e}"));
        assert!(g.get("20").is_none() && g.get("21").is_none());
        for n in ["6", "7"] {
            let inputs = g[n]["inputs"].as_object().unwrap_or_else(|| panic!("inputs"));
            assert!(inputs.contains_key("image1") && !inputs.contains_key("image2") && !inputs.contains_key("image3"), "{n}: {inputs:?}");
        }
        assert!(g.to_string().find("{{").is_none());
        // One reference: image2 stays, image3 goes.
        let g = t.fill(&bound(&t, &[("image", json!("in.png")), ("image2", json!("ref.png"))])).unwrap_or_else(|e| panic!("{e}"));
        assert_eq!(g["20"]["inputs"]["image"], json!("ref.png"));
        assert_eq!(g["6"]["inputs"]["image2"], json!(["20", 0]));
        assert!(g.get("21").is_none() && g["7"]["inputs"].get("image3").is_none());
        // The 2.1 edit prunes its grouped `images.image_N` keys the same way.
        let q = find("qwen-2.1/edit").unwrap_or_else(|e| panic!("{e}"));
        let g = q.fill(&bound(&q, &[("image", json!("in.png")), ("ref2", json!("a.png")), ("ref3", json!("b.png"))])).unwrap_or_else(|e| panic!("{e}"));
        let keys: Vec<&String> = g["6"]["inputs"].as_object().map(|m| m.keys().filter(|k| k.starts_with("images.")).collect()).unwrap_or_default();
        assert_eq!(keys.len(), 3, "{keys:?}");
        // Templates without references fill as before.
        let k = find("krea2-turbo/image").unwrap_or_else(|e| panic!("{e}"));
        assert_eq!(k.meta.references, References::default());
        assert!(k.fill(&bound(&k, &[("width", json!(64)), ("height", json!(64))])).is_ok());
    }

    #[test]
    fn every_template_says_where_it_shows_and_how_long_it_takes() {
        for t in builtin() {
            let m = &t.meta;
            assert!(!m.pages.is_empty(), "{}: pages", m.id);
            assert!(!m.creator.is_empty() && !m.description.is_empty(), "{}: creator / description", m.id);
            assert!(m.estimated_seconds.base > 0.0, "{}: estimate", m.id);
            assert!(m.counts.default >= 1 && m.counts.default <= m.counts.max, "{}: counts", m.id);
            if m.task == Task::Image {
                assert!(!m.sizes.aspects.is_empty() && m.sizes.min > 0 && m.sizes.max >= m.sizes.min && m.sizes.step > 0, "{}: sizes", m.id);
            }
            for p in &m.references.placeholders {
                assert!(m.references.nodes.contains_key(p), "{}: reference {p} lists its nodes", m.id);
            }
        }
        // The one formula: (base + per-MP × MP) × count.
        let t = find("qwen-edit-2511/fill-lightning-8").unwrap_or_else(|e| panic!("{e}"));
        assert!((t.meta.estimate_seconds(1000, 1000, 1, 0) - 13.0).abs() < 1e-3);
        assert!((t.meta.estimate_seconds(1000, 500, 2, 0) - 20.5).abs() < 1e-3);
        assert!((t.meta.estimate_seconds(0, 0, 0, 0) - 7.5).abs() < 1e-3, "count 0 counts as 1");
        // Each extra reference image adds its cost; a reference-driven text to image's first
        // reference stands in for the picture.
        let e = find("qwen-edit-2511/edit-lightning-8").unwrap_or_else(|e| panic!("{e}"));
        assert!((e.meta.estimate_seconds(1000, 1000, 1, 2) - (13.0 + 18.0)).abs() < 1e-3);
        let r = find("qwen-edit-2511/image-ref").unwrap_or_else(|e| panic!("{e}"));
        assert!((r.meta.estimate_seconds(1000, 1000, 1, 1) - 13.0).abs() < 1e-3);
        assert!((r.meta.estimate_seconds(1000, 1000, 1, 3) - 31.0).abs() < 1e-3);
    }

    #[test]
    fn bad_reference_metadata_is_rejected() {
        let src = find("qwen-edit-2511/edit").unwrap_or_else(|e| panic!("{e}"));
        let mut t = src.clone();
        t.meta.references.max = 3;
        assert!(t.check().is_err(), "more than the placeholders listed");
        let mut t = src.clone();
        t.meta.references.nodes.insert("image2".into(), vec!["99".into()]);
        assert!(t.check().is_err(), "a node not in the graph");
        let mut t = src;
        t.meta.references.min = 3;
        assert!(t.check().is_err(), "min above max");
    }

    #[test]
    fn malformed_files_are_rejected() {
        assert!(Template::parse("{").is_err());
        assert!(Template::parse(r#"{"meta":{"id":"x","name":"x","family":"x","task":"fill","license":"permissive","saveNode":"1"},"graph":{}}"#).is_err());
        // UI-format graphs (nodes array) are refused with a hint.
        let ui = r#"{"meta":{"id":"x","name":"x","family":"x","task":"fill","license":"permissive","saveNode":"1"},"graph":{"1":{"type":"KSampler"}}}"#;
        assert!(matches!(Template::parse(ui), Err(Error::Template(m)) if m.contains("API format")));
    }
}
