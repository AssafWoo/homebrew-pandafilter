//! Symbol and signature extraction from source files.
//!
//! Provides `apply_structural` which extracts function/struct/impl signatures
//! from source code, collapsing function bodies. Used by the focus indexer to
//! produce better BERT embeddings than raw first-N-chars — the API surface
//! (function names, types, struct fields) is what matters for relevance ranking.

// ── Language detection ─────────────────────────────────────────────────────

#[derive(Debug, Clone, PartialEq)]
enum Language {
    Rust,
    Python,
    TypeScript,
    Go,
    Java,
    CSharp,
    Cpp,
    Dart,
    Swift,
    Kotlin,
    Shell,
    DataFormat,
    Unknown,
}

fn detect_language(ext: &str) -> Language {
    match ext.to_lowercase().as_str() {
        "rs"                              => Language::Rust,
        "py" | "pyi"                      => Language::Python,
        "ts" | "tsx" | "js" | "jsx"
        | "mjs" | "cjs"                   => Language::TypeScript,
        "go"                              => Language::Go,
        "java"                            => Language::Java,
        "cs"                              => Language::CSharp,
        "cpp" | "cc" | "cxx" | "c"
        | "h" | "hpp"                     => Language::Cpp,
        "dart"                            => Language::Dart,
        "swift"                           => Language::Swift,
        "kt" | "kts"                      => Language::Kotlin,
        "sh" | "bash" | "zsh"            => Language::Shell,
        "json" | "yaml" | "yml"
        | "toml" | "xml" | "csv"         => Language::DataFormat,
        _                                 => Language::Unknown,
    }
}

// ── Signature detection ────────────────────────────────────────────────────

fn is_signature_line(trimmed: &str, lang: &Language) -> bool {
    match lang {
        Language::Rust => {
            trimmed.starts_with("pub ")
                || trimmed.starts_with("fn ")
                || trimmed.starts_with("struct ")
                || trimmed.starts_with("enum ")
                || trimmed.starts_with("impl ")
                || trimmed.starts_with("trait ")
                || trimmed.starts_with("type ")
                || trimmed.starts_with("use ")
                || trimmed.starts_with("mod ")
                || trimmed.starts_with("const ")
                || trimmed.starts_with("static ")
                || trimmed.starts_with("#[")
        }
        Language::Python => {
            trimmed.starts_with("def ")
                || trimmed.starts_with("class ")
                || trimmed.starts_with("import ")
                || trimmed.starts_with("from ")
                || trimmed.starts_with("async def ")
                || trimmed.starts_with('@')
        }
        Language::TypeScript => {
            trimmed.starts_with("export ")
                || trimmed.starts_with("import ")
                || trimmed.starts_with("function ")
                || trimmed.starts_with("class ")
                || trimmed.starts_with("interface ")
                || trimmed.starts_with("type ")
                || trimmed.starts_with("const ")
                || trimmed.starts_with("let ")
                || trimmed.starts_with("var ")
        }
        Language::Go => {
            trimmed.starts_with("func ")
                || trimmed.starts_with("type ")
                || trimmed.starts_with("import ")
                || trimmed.starts_with("var ")
                || trimmed.starts_with("const ")
                || trimmed.starts_with("package ")
        }
        Language::Dart => {
            trimmed.starts_with("class ")
                || trimmed.starts_with("abstract class ")
                || trimmed.starts_with("mixin ")
                || trimmed.starts_with("extension ")
                || trimmed.starts_with("enum ")
                || trimmed.starts_with("typedef ")
                || trimmed.starts_with("factory ")
                || trimmed.starts_with("static ")
                || trimmed.starts_with("final ")
                || trimmed.starts_with("const ")
                || trimmed.starts_with('@')
                || (trimmed.contains('(')
                    && !trimmed.starts_with("//")
                    && !trimmed.starts_with("*")
                    && !trimmed.starts_with("if ")
                    && !trimmed.starts_with("return ")
                    && !trimmed.starts_with("assert ")
                    && !trimmed.starts_with("throw ")
                    && !trimmed.starts_with("await ")
                    && !trimmed.starts_with("super(")
                    && !trimmed.starts_with("this("))
        }
        Language::Swift => {
            trimmed.starts_with("func ")
                || trimmed.starts_with("class ")
                || trimmed.starts_with("struct ")
                || trimmed.starts_with("enum ")
                || trimmed.starts_with("protocol ")
                || trimmed.starts_with("extension ")
                || trimmed.starts_with("typealias ")
                || trimmed.starts_with("var ")
                || trimmed.starts_with("let ")
                || trimmed.starts_with("static func ")
                || trimmed.starts_with("public func ")
                || trimmed.starts_with("private func ")
                || trimmed.starts_with("internal func ")
                || trimmed.starts_with("open func ")
                || trimmed.starts_with("override func ")
                || trimmed.starts_with("mutating func ")
                || trimmed.starts_with("init(")
                || trimmed.starts_with("convenience init(")
                || trimmed.starts_with('@')
                || trimmed.starts_with("import ")
        }
        Language::Kotlin => {
            trimmed.starts_with("fun ")
                || trimmed.starts_with("class ")
                || trimmed.starts_with("data class ")
                || trimmed.starts_with("sealed class ")
                || trimmed.starts_with("abstract class ")
                || trimmed.starts_with("open class ")
                || trimmed.starts_with("object ")
                || trimmed.starts_with("companion object")
                || trimmed.starts_with("interface ")
                || trimmed.starts_with("enum class ")
                || trimmed.starts_with("typealias ")
                || trimmed.starts_with("val ")
                || trimmed.starts_with("var ")
                || trimmed.starts_with("override fun ")
                || trimmed.starts_with("suspend fun ")
                || trimmed.starts_with("private fun ")
                || trimmed.starts_with("public fun ")
                || trimmed.starts_with("protected fun ")
                || trimmed.starts_with("internal fun ")
                || trimmed.starts_with("inline fun ")
                || trimmed.starts_with('@')
                || trimmed.starts_with("import ")
                || trimmed.starts_with("package ")
        }
        // Java/C#/Cpp: keep all non-empty (body collapsing handled via fn_like below)
        Language::Java | Language::CSharp | Language::Cpp => !trimmed.is_empty(),
        Language::Shell | Language::Unknown | Language::DataFormat => false,
    }
}

/// Returns true if this line looks like the start of a collapsible function body.
fn is_fn_like(trimmed: &str, lang: &Language) -> bool {
    match lang {
        Language::Rust => {
            trimmed.starts_with("fn ")
                || trimmed.starts_with("pub fn ")
                || trimmed.starts_with("pub(crate) fn ")
                || trimmed.starts_with("pub(super) fn ")
                || trimmed.starts_with("async fn ")
                || trimmed.starts_with("pub async fn ")
        }
        Language::Go => trimmed.starts_with("func "),
        Language::TypeScript => {
            trimmed.starts_with("function ")
                || trimmed.starts_with("export function ")
                || trimmed.starts_with("export async function ")
                || trimmed.starts_with("async function ")
                || trimmed.contains("=> {")
        }
        Language::Dart => {
            // Constructor / method: has parens, ends with `{`, not a control statement
            trimmed.contains('(')
                && trimmed.ends_with('{')
                && !trimmed.starts_with("if ")
                && !trimmed.starts_with("else ")
                && !trimmed.starts_with("} else")
                && !trimmed.starts_with("for ")
                && !trimmed.starts_with("while ")
                && !trimmed.starts_with("switch ")
                && !trimmed.starts_with("try ")
                && !trimmed.starts_with("//")
        }
        Language::Swift => {
            trimmed.starts_with("func ")
                || trimmed.starts_with("static func ")
                || trimmed.starts_with("public func ")
                || trimmed.starts_with("private func ")
                || trimmed.starts_with("internal func ")
                || trimmed.starts_with("open func ")
                || trimmed.starts_with("override func ")
                || trimmed.starts_with("mutating func ")
                || trimmed.starts_with("init(")
                || trimmed.starts_with("convenience init(")
        }
        Language::Kotlin => {
            trimmed.starts_with("fun ")
                || trimmed.starts_with("override fun ")
                || trimmed.starts_with("suspend fun ")
                || trimmed.starts_with("private fun ")
                || trimmed.starts_with("public fun ")
                || trimmed.starts_with("protected fun ")
                || trimmed.starts_with("internal fun ")
                || trimmed.starts_with("inline fun ")
        }
        Language::Java | Language::CSharp | Language::Cpp => {
            // Method: has parens, ends with `{`, not a control statement
            trimmed.contains('(')
                && trimmed.ends_with('{')
                && !trimmed.starts_with("if ")
                && !trimmed.starts_with("} else")
                && !trimmed.starts_with("else ")
                && !trimmed.starts_with("for ")
                && !trimmed.starts_with("while ")
                && !trimmed.starts_with("switch ")
                && !trimmed.starts_with("try ")
                && !trimmed.starts_with("catch ")
                && !trimmed.starts_with("synchronized ")
                && !trimmed.starts_with("//")
                && !trimmed.starts_with("*")
        }
        _ => false,
    }
}

// ── Structural extraction — brace-based languages ─────────────────────────

fn structural_brace(lines: &[&str], lang: &Language) -> Vec<String> {
    let mut result: Vec<String> = Vec::new();
    let mut depth: i32 = 0;
    let mut body_start: Option<usize> = None;
    let mut body_depth: i32 = 0;
    let mut collecting_doc = false;
    let mut doc_lines: Vec<String> = Vec::new();

    for (i, line) in lines.iter().enumerate() {
        let trimmed = line.trim();

        // Track doc comments (keep them for public items)
        let is_doc = trimmed.starts_with("///")
            || trimmed.starts_with("/**")
            || trimmed.starts_with("* ")
            || trimmed.starts_with("*/")
            || trimmed.starts_with("///");

        if is_doc && body_start.is_none() {
            collecting_doc = true;
            doc_lines.push(line.to_string());
            continue;
        }

        let opens  = line.chars().filter(|&c| c == '{').count() as i32;
        let closes = line.chars().filter(|&c| c == '}').count() as i32;

        if body_start.is_some() {
            depth += opens - closes;
            if depth < 0 { depth = 0; }

            if depth <= body_depth {
                // Body ended — emit collapse marker
                let body_lines = i.saturating_sub(body_start.unwrap());
                if body_lines > 0 {
                    result.push(format!(
                        "{}/* {} lines */",
                        " ".repeat(body_depth as usize * 4 + 4),
                        body_lines,
                    ));
                }
                result.push(line.to_string()); // closing brace
                body_start = None;
                collecting_doc = false;
                doc_lines.clear();
            }
            continue;
        }

        if is_signature_line(trimmed, lang) || depth == 0 {
            // Emit accumulated doc comments only for public items
            if collecting_doc
                && (trimmed.starts_with("pub ")
                    || trimmed.starts_with("export ")
                    || trimmed.starts_with("public "))
            {
                for dl in &doc_lines {
                    result.push(dl.clone());
                }
            }
            collecting_doc = false;
            doc_lines.clear();

            // Type definitions: keep fully (struct/enum fields are the API)
            let is_type_def = trimmed.starts_with("struct ")
                || trimmed.starts_with("pub struct ")
                || trimmed.starts_with("enum ")
                || trimmed.starts_with("pub enum ")
                || trimmed.starts_with("enum class ")  // Kotlin
                || trimmed.starts_with("interface ")
                || trimmed.starts_with("export interface ")
                || trimmed.starts_with("type ")
                || trimmed.starts_with("pub type ");

            if is_type_def {
                result.push(line.to_string());
                depth += opens - closes;
                if depth < 0 { depth = 0; }
                continue;
            }

            result.push(line.to_string());
            depth += opens - closes;
            if depth < 0 { depth = 0; }

            if is_fn_like(trimmed, lang) && opens > closes {
                body_start = Some(i);
                body_depth = depth - (opens - closes);
            }
        } else {
            // Inside a block (e.g. impl body, class body) — emit as-is, watch for methods
            result.push(line.to_string());
            depth += opens - closes;
            if depth < 0 { depth = 0; }

            if is_fn_like(trimmed, lang) && opens > closes {
                body_start = Some(i);
                body_depth = depth - (opens - closes);
            }
        }
    }

    result
}

// ── Structural extraction — Python (indentation-based) ────────────────────

fn structural_python(lines: &[&str]) -> Vec<String> {
    let mut result: Vec<String> = Vec::new();
    let mut i = 0;

    while i < lines.len() {
        let trimmed = lines[i].trim();
        let indent = lines[i].len() - lines[i].trim_start().len();

        // Keep imports, class definitions, decorators
        if trimmed.starts_with("import ")
            || trimmed.starts_with("from ")
            || trimmed.starts_with("class ")
            || trimmed.starts_with('@')
        {
            result.push(lines[i].to_string());
            i += 1;
            continue;
        }

        // Function/method: keep signature, collapse body
        if trimmed.starts_with("def ") || trimmed.starts_with("async def ") {
            result.push(lines[i].to_string());
            let body_indent = indent + 4;
            let body_start = i + 1;
            i += 1;

            while i < lines.len() {
                let next_trimmed = lines[i].trim();
                let next_indent   = lines[i].len() - lines[i].trim_start().len();
                if next_trimmed.is_empty() { i += 1; continue; }
                if next_indent >= body_indent { i += 1; } else { break; }
            }

            let body_lines = i.saturating_sub(body_start);
            if body_lines > 0 {
                let pad = " ".repeat(body_indent);
                result.push(format!("{}# ... {} lines ...", pad, body_lines));
            }
            continue;
        }

        // Top-level statements: keep
        if indent == 0 && !trimmed.is_empty() {
            result.push(lines[i].to_string());
        }

        i += 1;
    }

    result
}

// ── Vue / Svelte — extract <script> block, process as TypeScript ──────────

fn extract_script_block(content: &str) -> Option<String> {
    // Match <script>, <script lang="ts">, <script setup>, etc.
    let lower = content.to_lowercase();
    let tag_start = lower.find("<script")?;
    let tag_end = content[tag_start..].find('>')?;
    let script_start = tag_start + tag_end + 1;
    let after = &content[script_start..];
    let script_end = after.to_lowercase().find("</script>")?;
    Some(after[..script_end].to_string())
}

// ── Public API ─────────────────────────────────────────────────────────────

/// Extract structural signatures from source code for embedding.
///
/// Returns function/struct/impl/class signatures with function bodies collapsed.
/// Struct/enum fields are kept in full (they are the type's API).
///
/// `path` is used for language detection via file extension.
///
/// Returns an empty string for unsupported types (data formats, shell scripts,
/// unknown extensions). Callers should fall back to raw content in that case.
pub fn apply_structural(content: &str, path: &str) -> String {
    let lower_path = path.to_lowercase();
    let ext = path.rfind('.').map(|i| &path[i + 1..]).unwrap_or("");

    // Vue / Svelte: extract <script> block and process as TypeScript
    if lower_path.ends_with(".vue") || lower_path.ends_with(".svelte") {
        let script = extract_script_block(content).unwrap_or_default();
        if script.trim().is_empty() {
            return String::new();
        }
        let lines: Vec<&str> = script.lines().collect();
        return structural_brace(&lines, &Language::TypeScript).join("\n");
    }

    let lang = detect_language(ext);

    match lang {
        Language::DataFormat | Language::Shell | Language::Unknown => return String::new(),
        _ => {}
    }

    let lines: Vec<&str> = content.lines().collect();
    let result = if lang == Language::Python {
        structural_python(&lines)
    } else {
        structural_brace(&lines, &lang)
    };
    result.join("\n")
}

// ── Tests ──────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rust_fn_bodies_collapsed() {
        let content = "pub fn hello() {\n    println!(\"hello\");\n    let x = 42;\n}\n\npub fn world() -> String {\n    String::from(\"world\")\n}\n";
        let out = apply_structural(content, "src/lib.rs");
        assert!(out.contains("pub fn hello()"),   "fn sig kept");
        assert!(out.contains("pub fn world()"),   "fn sig kept");
        assert!(!out.contains("println!"),        "body collapsed");
        assert!(!out.contains("String::from"),    "body collapsed");
        assert!(out.contains("/* "),              "collapse marker present");
    }

    #[test]
    fn struct_fields_preserved() {
        let content = "pub struct Foo {\n    pub name: String,\n    pub count: usize,\n}\n";
        let out = apply_structural(content, "src/foo.rs");
        assert!(out.contains("pub struct Foo"),   "struct kept");
        assert!(out.contains("pub name: String"), "field kept");
        assert!(out.contains("pub count: usize"), "field kept");
    }

    #[test]
    fn impl_block_methods_collapsed() {
        let content = "impl Foo {\n    pub fn new() -> Self {\n        Foo { count: 0 }\n    }\n}\n";
        let out = apply_structural(content, "src/foo.rs");
        assert!(out.contains("impl Foo"),         "impl kept");
        assert!(out.contains("pub fn new()"),     "method sig kept");
        assert!(!out.contains("Foo { count: 0}"), "body collapsed");
    }

    #[test]
    fn python_def_collapsed() {
        let content = "def greet(name):\n    print(name)\n    return True\n\nclass Foo:\n    pass\n";
        let out = apply_structural(content, "app.py");
        assert!(out.contains("def greet(name):"), "fn sig kept");
        assert!(!out.contains("print(name)"),     "body collapsed");
        assert!(out.contains("class Foo:"),        "class kept");
    }

    #[test]
    fn kotlin_fn_collapsed() {
        let content = "package com.example\n\nfun greet(name: String): String {\n    return \"Hello $name\"\n}\n\ndata class User(val id: Int, val name: String)\n";
        let out = apply_structural(content, "Main.kt");
        assert!(out.contains("fun greet("),       "fn sig kept");
        assert!(!out.contains("Hello $name"),     "body collapsed");
        assert!(out.contains("data class User"),  "data class kept");
    }

    #[test]
    fn swift_fn_collapsed() {
        let content = "import Foundation\n\nfunc buildRequest(url: URL) -> URLRequest {\n    var req = URLRequest(url: url)\n    return req\n}\n\nstruct Router {\n    var routes: [Route]\n}\n";
        let out = apply_structural(content, "Router.swift");
        assert!(out.contains("func buildRequest("),  "fn sig kept");
        assert!(!out.contains("URLRequest(url: url)"), "body collapsed");
        assert!(out.contains("struct Router"),       "struct kept");
    }

    #[test]
    fn java_method_collapsed() {
        let content = "public class PetClinicApplication {\n    public static void main(String[] args) {\n        SpringApplication.run(PetClinicApplication.class, args);\n    }\n}\n";
        let out = apply_structural(content, "PetClinicApplication.java");
        assert!(out.contains("public class PetClinicApplication"), "class kept");
        assert!(out.contains("public static void main("),         "method sig kept");
        assert!(!out.contains("SpringApplication.run"),           "body collapsed");
    }

    #[test]
    fn csharp_method_collapsed() {
        let content = "public class LoggerConfiguration {\n    public LoggerConfiguration WriteTo(ILogEventSink sink) {\n        sinks.Add(sink);\n        return this;\n    }\n}\n";
        let out = apply_structural(content, "LoggerConfiguration.cs");
        assert!(out.contains("public class LoggerConfiguration"),    "class kept");
        assert!(out.contains("public LoggerConfiguration WriteTo("), "method sig kept");
        assert!(!out.contains("sinks.Add"),                          "body collapsed");
    }

    #[test]
    fn vue_script_extracted() {
        let content = "<template><div>hello</div></template>\n<script setup>\nimport { ref } from 'vue'\nexport function useFoo() {\n  const x = ref(0)\n  return x\n}\n</script>\n";
        let out = apply_structural(content, "Foo.vue");
        assert!(out.contains("export function useFoo"), "script fn sig kept");
        assert!(!out.contains("ref(0)"),                "body collapsed");
        assert!(!out.contains("<template>"),            "template excluded");
    }

    #[test]
    fn dataformat_returns_empty() {
        let out = apply_structural(r#"{"key": "value"}"#, "config.json");
        assert!(out.is_empty(), "data format → empty (caller falls back)");
    }

    #[test]
    fn unknown_ext_returns_empty() {
        let out = apply_structural("some content", "file.wasm");
        assert!(out.is_empty());
    }

    #[test]
    fn typescript_exported_fn_collapsed() {
        let content = "export function fetchUser(id: string): Promise<User> {\n    return api.get(`/users/${id}`);\n}\n\nexport interface User {\n    id: string;\n    name: string;\n}\n";
        let out = apply_structural(content, "src/api.ts");
        assert!(out.contains("export function fetchUser"), "fn sig kept");
        assert!(!out.contains("api.get"),                 "body collapsed");
        assert!(out.contains("export interface User"),    "interface kept");
        assert!(out.contains("id: string"),               "interface field kept");
    }

    #[test]
    fn go_func_collapsed() {
        let content = "package main\n\nfunc main() {\n    fmt.Println(\"hello\")\n}\n\nfunc add(a, b int) int {\n    return a + b\n}\n";
        let out = apply_structural(content, "main.go");
        assert!(out.contains("func main()"),   "fn sig kept");
        assert!(out.contains("func add("),     "fn sig kept");
        assert!(!out.contains("fmt.Println"),  "body collapsed");
        assert!(!out.contains("return a + b"), "body collapsed");
    }
}
