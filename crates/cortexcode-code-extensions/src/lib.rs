//! WASM extension system for the cortex coding agent.
//!
//! This crate is the initial v2 plugin implementation described in the
//! migration guide: portable, sandboxed WASM plugins via `wasmtime`. The guest
//! ABI is intentionally tiny so that plugins can be written in any language
//! that compiles to WASM:
//!
//! * Guest exports `memory`.
//! * Guest exports `alloc(size: i32) -> i32` and `dealloc(ptr: i32, size: i32)`.
//! * Guest exports `run(input_ptr: i32, input_len: i32) -> i32` returning the
//!   pointer to an output buffer. The buffer is read and freed by the host.
//!
//! The host provides a `log` import so guests can surface diagnostics.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use wasmtime::{Caller, Engine, Extern, Linker, Memory, MemoryType, Module, Store, TypedFunc};

// ---------------------------------------------------------------------------
// Manifest
// ---------------------------------------------------------------------------

/// Plugin metadata loaded from a JSON manifest.
#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq)]
#[serde(default)]
pub struct PluginManifest {
    /// Plugin name.
    pub name: String,
    /// Semantic version.
    pub version: String,
    /// Path to the `.wasm` binary, relative to the manifest directory.
    pub wasm: PathBuf,
    /// Human-readable description.
    pub description: String,
    /// Extra metadata.
    #[serde(default, flatten)]
    pub extra: HashMap<String, serde_json::Value>,
}

// ---------------------------------------------------------------------------
// Errors
// ---------------------------------------------------------------------------

/// Errors that can occur while loading or running a plugin.
#[derive(Debug)]
pub enum PluginError {
    Io(std::io::Error),
    Json(serde_json::Error),
    Wasm(wasmtime::Error),
    NotFound(PathBuf),
    MissingExport(String),
    InvalidUtf8,
    Runtime(String),
}

impl std::fmt::Display for PluginError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            PluginError::Io(e) => write!(f, "io error: {}", e),
            PluginError::Json(e) => write!(f, "json error: {}", e),
            PluginError::Wasm(e) => write!(f, "wasm error: {}", e),
            PluginError::NotFound(p) => write!(f, "plugin not found: {}", p.display()),
            PluginError::MissingExport(e) => write!(f, "missing export: {}", e),
            PluginError::InvalidUtf8 => write!(f, "plugin returned invalid utf-8"),
            PluginError::Runtime(e) => write!(f, "runtime error: {}", e),
        }
    }
}

impl std::error::Error for PluginError {}

impl From<std::io::Error> for PluginError {
    fn from(e: std::io::Error) -> Self {
        PluginError::Io(e)
    }
}

impl From<serde_json::Error> for PluginError {
    fn from(e: serde_json::Error) -> Self {
        PluginError::Json(e)
    }
}

impl From<wasmtime::Error> for PluginError {
    fn from(e: wasmtime::Error) -> Self {
        PluginError::Wasm(e)
    }
}

// ---------------------------------------------------------------------------
// Plugin
// ---------------------------------------------------------------------------

/// A loaded WASM plugin instance.
#[derive(Debug)]
pub struct Plugin {
    manifest: PluginManifest,
}

impl Plugin {
    /// Load a plugin manifest from a path. The `.wasm` binary is not yet
    /// instantiated; use `instantiate` to run it.
    pub fn load_manifest(path: impl AsRef<Path>) -> Result<Self, PluginError> {
        let path = path.as_ref();
        if !path.exists() {
            return Err(PluginError::NotFound(path.to_path_buf()));
        }
        let content = std::fs::read_to_string(path)?;
        let manifest: PluginManifest = serde_json::from_str(&content)?;
        Ok(Self { manifest })
    }

    /// Return the manifest.
    pub fn manifest(&self) -> &PluginManifest {
        &self.manifest
    }

    /// Resolve the wasm binary path relative to a base directory.
    pub fn wasm_path(&self, base_dir: impl AsRef<Path>) -> PathBuf {
        base_dir.as_ref().join(&self.manifest.wasm)
    }

    /// Instantiate the plugin and call its `run` export with the given JSON
    /// input.
    ///
    /// The guest ABI is described in the crate-level documentation.
    pub fn run(&self, wasm_path: impl AsRef<Path>, input: &str) -> Result<String, PluginError> {
        let wasm_path = wasm_path.as_ref();
        if !wasm_path.exists() {
            return Err(PluginError::NotFound(wasm_path.to_path_buf()));
        }

        let engine = Engine::default();
        let module = Module::from_file(&engine, wasm_path)?;
        let mut linker = Linker::<HostState>::new(&engine);
        let mut store = Store::new(&engine, HostState::default());

        // Provide a small page of memory for the host to use when the guest
        // has not yet exported its own memory.
        let host_memory = Memory::new(&mut store, MemoryType::new(1, Some(1)))?;
        linker.define(&mut store, "env", "memory", host_memory)?;

        linker.func_wrap(
            "env",
            "log",
            |mut caller: Caller<'_, HostState>, ptr: i32, len: i32| {
                let memory = memory_from_caller(&mut caller);
                let bytes = read_memory(&memory, &caller, ptr, len);
                if let Ok(text) = std::str::from_utf8(&bytes) {
                    caller.data_mut().logs.push(text.to_string());
                }
            },
        )?;

        let instance = linker.instantiate(&mut store, &module)?;

        let memory = instance
            .get_memory(&mut store, "memory")
            .ok_or_else(|| PluginError::MissingExport("memory".to_string()))?;

        let alloc: TypedFunc<i32, i32> = instance
            .get_typed_func(&mut store, "alloc")
            .map_err(|_| PluginError::MissingExport("alloc".to_string()))?;
        let dealloc: TypedFunc<(i32, i32), ()> = instance
            .get_typed_func(&mut store, "dealloc")
            .map_err(|_| PluginError::MissingExport("dealloc".to_string()))?;
        let run: TypedFunc<(i32, i32), i32> = instance
            .get_typed_func(&mut store, "run")
            .map_err(|_| PluginError::MissingExport("run".to_string()))?;

        let input_bytes = input.as_bytes();
        let input_ptr = alloc.call(&mut store, input_bytes.len() as i32)?;
        write_memory(&memory, &mut store, input_ptr, input_bytes)?;

        let output_ptr = run.call(&mut store, (input_ptr, input_bytes.len() as i32))?;
        // Read the length of the output buffer from the first 4 bytes.
        let len_bytes = read_memory(&memory, &store, output_ptr, 4);
        let output_len =
            i32::from_le_bytes([len_bytes[0], len_bytes[1], len_bytes[2], len_bytes[3]]) as usize;
        let output_bytes = read_memory(&memory, &store, output_ptr + 4, output_len as i32);
        let output = std::str::from_utf8(&output_bytes)
            .map_err(|_| PluginError::InvalidUtf8)?
            .to_string();

        dealloc.call(&mut store, (input_ptr, input_bytes.len() as i32))?;
        dealloc.call(&mut store, (output_ptr, (output_len + 4) as i32))?;

        Ok(output)
    }
}

// ---------------------------------------------------------------------------
// Host state
// ---------------------------------------------------------------------------

#[derive(Debug, Default)]
struct HostState {
    logs: Vec<String>,
}

fn memory_from_caller(caller: &mut Caller<'_, HostState>) -> Memory {
    caller
        .get_export("memory")
        .and_then(Extern::into_memory)
        .expect("memory export")
}

fn read_memory(memory: &Memory, store: impl wasmtime::AsContext, ptr: i32, len: i32) -> Vec<u8> {
    let mut buf = vec![0u8; len as usize];
    let _ = memory.read(&store, ptr as usize, &mut buf);
    buf
}

fn write_memory(
    memory: &Memory,
    mut store: impl wasmtime::AsContextMut,
    ptr: i32,
    data: &[u8],
) -> Result<(), PluginError> {
    memory
        .write(&mut store, ptr as usize, data)
        .map_err(|e| PluginError::Wasm(e.into()))
}

// ---------------------------------------------------------------------------
// Registry
// ---------------------------------------------------------------------------

/// A registry of loaded plugin manifests.
#[derive(Debug, Default)]
pub struct PluginRegistry {
    plugins: HashMap<String, Plugin>,
}

impl PluginRegistry {
    /// Create an empty registry.
    pub fn new() -> Self {
        Self::default()
    }

    /// Load all plugin manifests from a directory.
    pub fn load_dir(&mut self, path: impl AsRef<Path>) -> Result<(), PluginError> {
        let path = path.as_ref();
        if !path.exists() {
            return Ok(());
        }
        for entry in walkdir::WalkDir::new(path)
            .max_depth(1)
            .into_iter()
            .filter_map(|e| e.ok())
        {
            if entry.file_type().is_file()
                && entry.path().extension() == Some(std::ffi::OsStr::new("json"))
            {
                let plugin = Plugin::load_manifest(entry.path())?;
                let name = plugin.manifest().name.clone();
                self.plugins.insert(name, plugin);
            }
        }
        Ok(())
    }

    /// Register a plugin manually.
    pub fn register(&mut self, plugin: Plugin) {
        let name = plugin.manifest().name.clone();
        self.plugins.insert(name, plugin);
    }

    /// Look up a plugin by name.
    pub fn get(&self, name: &str) -> Option<&Plugin> {
        self.plugins.get(name)
    }

    /// Return all plugin names.
    pub fn names(&self) -> Vec<&str> {
        let mut names: Vec<_> = self.plugins.keys().map(|s| s.as_str()).collect();
        names.sort();
        names
    }

    /// Return the number of registered plugins.
    pub fn len(&self) -> usize {
        self.plugins.len()
    }

    pub fn is_empty(&self) -> bool {
        self.plugins.is_empty()
    }
}

/// Default directory where plugins are stored relative to the current working
/// directory.
pub fn default_plugins_dir() -> PathBuf {
    PathBuf::from(".cortexcode/plugins")
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_manifest_load() {
        let dir = std::env::temp_dir().join(format!("cortex-ext-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join("hello.json"),
            r#"{"name":"hello","version":"1.0.0","wasm":"hello.wasm","description":"test"}"#,
        )
        .unwrap();
        let plugin = Plugin::load_manifest(dir.join("hello.json")).unwrap();
        assert_eq!(plugin.manifest().name, "hello");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_registry_load_dir() {
        let dir = std::env::temp_dir().join(format!("cortex-ext-reg-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join("a.json"),
            r#"{"name":"a","version":"1.0.0","wasm":"a.wasm"}"#,
        )
        .unwrap();
        std::fs::write(
            dir.join("b.json"),
            r#"{"name":"b","version":"1.0.0","wasm":"b.wasm"}"#,
        )
        .unwrap();
        let mut registry = PluginRegistry::new();
        registry.load_dir(&dir).unwrap();
        assert_eq!(registry.len(), 2);
        assert!(registry.get("a").is_some());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_manifest_not_found() {
        let path = PathBuf::from("/this/should/not/exist/manifest.json");
        assert!(matches!(
            Plugin::load_manifest(&path),
            Err(PluginError::NotFound(_))
        ));
    }

    #[test]
    fn test_run_wasm_plugin() {
        // WAT for a plugin that allocates an input buffer, reads it, and writes
        // a length-prefixed greeting back.
        let wat = r#"
            (module
              (memory (export "memory") 2)
              (func (export "alloc") (param i32) (result i32)
                i32.const 1024)
              (func (export "dealloc") (param i32 i32))
              (func (export "run") (param i32 i32) (result i32)
                (local $i i32)
                i32.const 2048
                i32.const 7
                i32.store
                (block
                  (loop
                    local.get $i
                    local.get 1
                    i32.ge_u
                    br_if 1
                    i32.const 2052
                    local.get $i
                    i32.add
                    i32.const 1024
                    local.get $i
                    i32.add
                    i32.load8_u
                    i32.store8
                    local.get $i
                    i32.const 1
                    i32.add
                    local.set $i
                    br 0))
                i32.const 2048)
              (data (i32.const 3072) "hello, ")
            )
        "#;

        let dir = std::env::temp_dir().join(format!("cortex-ext-wasm-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let wasm_path = dir.join("plugin.wasm");
        std::fs::write(&wasm_path, wat).unwrap();

        let plugin = Plugin {
            manifest: PluginManifest {
                name: "echo".to_string(),
                version: "1.0.0".to_string(),
                wasm: wasm_path.clone(),
                description: "echo".to_string(),
                extra: HashMap::new(),
            },
        };

        let result = plugin.run(&wasm_path, "world");
        let _ = std::fs::remove_dir_all(&dir);
        assert!(result.is_ok(), "{:?}", result);
        assert!(result.unwrap().contains("world"));
    }
}
