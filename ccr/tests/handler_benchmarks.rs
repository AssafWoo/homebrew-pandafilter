//! Handler benchmark tests — realistic large-project fixtures.
//!
//! Each benchmark feeds a realistic command output through the handler and compares
//! token counts before (what Claude sees without PandaFilter) and after (what Claude sees with PandaFilter).
//!
//! Run with:
//!   cargo test -p panda benchmark -- --nocapture
//!
//! For git status / git log / cargo build the "without PandaFilter" baseline is the command's
//! native verbose output; the handler receives the flag-rewritten form (porcelain /
//! oneline / --message-format json) and compresses it further. The combination is
//! the true end-to-end savings a user gets after `panda init`.

use panda::handlers::{
    biome::BiomeHandler,
    brew::BrewHandler,
    cargo::CargoHandler,
    clippy::ClippyHandler,
    docker::DockerHandler,
    ember::EmberHandler,
    env::EnvHandler,
    eslint::EslintHandler,
    gh::GhHandler,
    git::GitHandler,
    go::GoHandler,
    golangci_lint::GolangCiLintHandler,
    grep::GrepHandler,
    helm::HelmHandler,
    jest::JestHandler,
    kubectl::KubectlHandler,
    ls::LsHandler,
    make::MakeHandler,
    maven::{GradleHandler, MavenHandler},
    mypy::MypyHandler,
    next::NextHandler,
    npm::NpmHandler,
    nx::NxHandler,
    pip::PipHandler,
    playwright::PlaywrightHandler,
    pytest::PytestHandler,
    ruff::RuffHandler,
    stylelint::StylelintHandler,
    terraform::TerraformHandler,
    tsc::TscHandler,
    turbo::TurboHandler,
    uv::UvHandler,
    vitest::VitestHandler,
    vite::ViteHandler,
    webpack::WebpackHandler,
    Handler,
};
use panda_core::tokens::count_tokens;

// ─── helpers ─────────────────────────────────────────────────────────────────

fn savings_pct(in_tok: usize, out_tok: usize) -> f64 {
    if in_tok == 0 { return 0.0; }
    (in_tok - out_tok) as f64 / in_tok as f64 * 100.0
}

fn run(handler: &dyn Handler, handler_input: &str, args: &[&str]) -> String {
    let args: Vec<String> = args.iter().map(|s| s.to_string()).collect();
    handler.filter(handler_input, &args)
}

// ─── fixtures ────────────────────────────────────────────────────────────────

/// `cargo build` with 130 real crate names and 5 warnings.
/// baseline  = human-readable stdout (what Claude sees without CCR)
/// handler_input = --message-format json (what filter() receives after arg rewrite)
fn cargo_build() -> (String /* baseline */, String /* handler_input */) {
    let deps = [
        "proc-macro2","unicode-ident","syn","quote","serde","serde_derive",
        "serde_json","itoa","ryu","indexmap","hashbrown","ahash","zerocopy",
        "once_cell","lazy_static","regex","regex-syntax","aho-corasick","memchr",
        "bytes","tokio","tokio-macros","mio","socket2","libc","pin-project-lite",
        "futures-core","futures-util","futures-sink","futures-task","pin-utils",
        "slab","async-trait","tower","tower-layer","tower-service",
        "hyper","http","http-body","httparse","h2","want",
        "tracing","tracing-core","tracing-subscriber","tracing-attributes","valuable",
        "log","env_logger","humantime","termcolor","atty",
        "clap","clap_derive","clap_lex","clap_builder","strsim",
        "anstream","anstyle","anstyle-parse","anstyle-query","colorchoice","utf8parse",
        "anyhow","thiserror","thiserror-impl","dirs","dirs-sys",
        "cfg-if","bitflags","nix","rustix","linux-raw-sys","errno",
        "tempfile","rand","rand_core","rand_chacha","ppv-lite86","getrandom",
        "uuid","hex","base64","url","percent-encoding","idna",
        "unicode-normalization","unicode-bidi","form_urlencoded","tinyvec",
        "reqwest","rustls","rustls-webpki","ring","spin","untrusted",
        "openssl","openssl-sys","foreign-types","foreign-types-shared",
        "native-tls","security-framework","security-framework-sys",
        "core-foundation","core-foundation-sys","cc","pkg-config",
        "chrono","num-integer","num-traits","iana-time-zone",
        "time","time-macros","deranged","powerfmt",
        "sqlx","sqlx-core","sqlx-macros","sqlx-postgres","sqlx-sqlite","dotenvy","heck",
        "axum","axum-core","matchit","mime","mime_guess","encoding_rs",
        "tower-http","hyper-util","http-body-util",
        "myapp",
    ];

    // ── baseline: human-readable (no flag rewriting) ──
    let mut baseline = String::new();
    for dep in &deps {
        baseline.push_str(&format!("   Compiling {} v1.0.0\n", dep));
    }
    baseline.push_str(concat!(
        "warning: unused variable: `conn`\n",
        " --> src/db/pool.rs:87:9\n",
        "  |\n",
        "87|     let conn = pool.acquire().await?;\n",
        "  |         ^^^^ help: if this is intentional, prefix it with an underscore: `_conn`\n",
        "  |\n",
        "  = note: `#[warn(unused_variables)]` on by default\n\n",
        "warning: unused variable: `config`\n",
        " --> src/server.rs:23:9\n",
        "  |\n",
        "23|     let config = AppConfig::load()?;\n",
        "  |         ^^^^^^ help: prefix with an underscore: `_config`\n\n",
        "warning: unused variable: `req`\n",
        " --> src/middleware/auth.rs:45:9\n",
        "  |\n",
        "45|     let req = request.into_parts();\n",
        "  |         ^^^ help: prefix with an underscore: `_req`\n\n",
        "warning: function is never used: `legacy_handler`\n",
        " --> src/api/v1.rs:120:4\n",
        "  |\n",
        "120| fn legacy_handler() {}\n",
        "   | ^^^^^^^^^^^^^^^^^^^^^^\n",
        "  |\n",
        "  = note: `#[warn(dead_code)]` on by default\n\n",
        "warning: unreachable expression\n",
        " --> src/handlers/webhook.rs:67:9\n",
        "  |\n",
        "67|     return Ok(());\n",
        "68|     log::info!(\"done\");\n",
        "   |     ^^^^^^^^^^^^^^^^^ unreachable expression\n\n",
        "warning: `myapp` (bin \"myapp\") generated 5 warnings\n",
        "    Finished `dev` profile [unoptimized + debuginfo] target(s) in 87.45s\n",
    ));

    // ── handler_input: --message-format json ──
    let mut h = String::new();
    for dep in deps.iter().take(deps.len() - 1) {
        h.push_str(&format!(
            "{{\"reason\":\"compiler-artifact\",\"package_id\":\"{dep} 1.0.0 \
             (registry+https://github.com/rust-lang/crates.io-index)\",\
             \"target\":{{\"kind\":[\"lib\"],\"name\":\"{dep}\",\"src_path\":\
             \"/home/user/.cargo/registry/src/{dep}/src/lib.rs\"}},\
             \"profile\":{{\"opt_level\":\"0\",\"debuginfo\":2}},\
             \"features\":[],\"filenames\":[\"/path/to/lib{dep}.rlib\"],\"fresh\":false}}\n",
            dep = dep
        ));
    }
    let warnings = [
        ("unused_variables", "unused variable: `conn`",              "src/db/pool.rs",          87),
        ("unused_variables", "unused variable: `config`",            "src/server.rs",           23),
        ("unused_variables", "unused variable: `req`",               "src/middleware/auth.rs",  45),
        ("dead_code",        "function is never used: `legacy_handler`", "src/api/v1.rs",       120),
        ("unreachable_code", "unreachable expression",               "src/handlers/webhook.rs", 67),
    ];
    for (code, msg, file, line) in &warnings {
        h.push_str(&format!(
            "{{\"reason\":\"compiler-message\",\
             \"package_id\":\"myapp 0.1.0 (path+file:///path/to/myapp)\",\
             \"target\":{{\"kind\":[\"bin\"],\"name\":\"myapp\"}},\
             \"message\":{{\"message\":\"{msg}\",\"level\":\"warning\",\
             \"spans\":[{{\"file_name\":\"{file}\",\"line_start\":{line}}}],\
             \"code\":{{\"code\":\"{code}\"}},\"rendered\":\"warning: {msg}...\"}}}}\n",
            msg = msg, file = file, line = line, code = code
        ));
    }
    h.push_str("{\"reason\":\"build-finished\",\"success\":true}\n");

    (baseline, h)
}

/// `cargo test` — 198 passing, 2 failures.
fn cargo_test() -> String {
    let mut out = String::new();
    let modules = [
        "api", "auth", "db", "handlers", "middleware",
        "models", "utils", "config", "services",
    ];
    let mut n = 0usize;
    for module in &modules {
        for i in 0..25usize {
            out.push_str(&format!(
                "test {}::tests::test_{}_case_{:02} ... ok\n", module, module, i
            ));
            n += 1;
            if n >= 198 { break; }
        }
        if n >= 198 { break; }
    }
    out.push_str("test auth::tests::test_jwt_expiry ... FAILED\n");
    out.push_str("test db::tests::test_pool_overflow ... FAILED\n");
    out.push_str("\nfailures:\n\n");
    out.push_str("---- auth::tests::test_jwt_expiry stdout ----\n");
    out.push_str("thread 'auth::tests::test_jwt_expiry' panicked at \
                  'assertion failed: token.is_valid()'\n");
    out.push_str("src/auth/jwt.rs:156:9\n");
    out.push_str("note: run with `RUST_BACKTRACE=1` for a backtrace\n\n");
    out.push_str("---- db::tests::test_pool_overflow stdout ----\n");
    out.push_str("thread 'db::tests::test_pool_overflow' panicked at \
                  'called `Result::unwrap()` on an `Err` value: PoolTimedOut'\n");
    out.push_str("src/db/pool.rs:89:14\n\n");
    out.push_str("failures:\n");
    out.push_str("    auth::tests::test_jwt_expiry\n");
    out.push_str("    db::tests::test_pool_overflow\n\n");
    out.push_str(
        "test result: FAILED. 198 passed; 2 failed; 0 ignored; finished in 14.32s\n",
    );
    out
}

/// `git status` — verbose baseline + porcelain handler input.
fn git_status() -> (String /* baseline */, String /* porcelain */) {
    let staged = [
        "src/auth/login.ts", "src/auth/logout.ts", "src/auth/middleware.ts",
        "src/api/users.ts",  "src/api/posts.ts",   "src/api/comments.ts",
        "src/models/user.ts","src/models/post.ts", "src/services/auth.ts",
        "src/config/database.ts",
    ];
    let modified = [
        "src/api/health.ts",         "src/api/metrics.ts",
        "src/components/Button.tsx", "src/components/Modal.tsx",
        "src/components/Form.tsx",   "src/components/Table.tsx",
        "src/components/Header.tsx", "src/components/Footer.tsx",
        "src/components/Sidebar.tsx","src/components/Dashboard.tsx",
        "src/pages/Home.tsx",        "src/pages/Login.tsx",
        "src/pages/Register.tsx",    "src/pages/Profile.tsx",
        "src/pages/Settings.tsx",    "src/hooks/useAuth.ts",
        "src/hooks/useUser.ts",      "src/hooks/usePosts.ts",
        "src/store/auth.ts",         "src/store/posts.ts",
        "src/store/ui.ts",           "src/utils/api.ts",
        "src/utils/format.ts",       "src/utils/validate.ts",
        "src/utils/storage.ts",      "src/utils/errors.ts",
        "tests/auth.test.ts",        "tests/api.test.ts",
        "tests/components.test.tsx", "package.json",
        "tsconfig.json",             "jest.config.ts",
        "src/styles/globals.css",    "src/styles/components.css",
        "src/constants/routes.ts",   "src/constants/api.ts",
        "src/types/user.ts",         "src/types/post.ts",
        "src/types/api.ts",          "src/types/ui.ts",
    ];
    let untracked = [
        "src/components/NewWidget.tsx",
        "src/pages/Analytics.tsx",
        "src/hooks/useAnalytics.ts",
        "src/utils/logger.ts",
        "src/services/analytics.ts",
        "src/types/analytics.ts",
        "migrations/20240318_add_analytics.sql",
        "docs/ANALYTICS.md",
    ];

    let mut baseline = String::new();
    baseline.push_str("On branch feature/user-auth\n");
    baseline.push_str("Your branch is ahead of 'origin/feature/user-auth' by 3 commits.\n");
    baseline.push_str("  (use \"git push\" to publish your local commits)\n\n");
    baseline.push_str("Changes to be committed:\n");
    baseline.push_str("  (use \"git restore --staged <file>...\" to unstage)\n");
    for f in &staged   { baseline.push_str(&format!("\tmodified:   {}\n", f)); }
    baseline.push_str("\nChanges not staged for commit:\n");
    baseline.push_str("  (use \"git restore <file>...\" to update what will be committed)\n");
    baseline.push_str("  (use \"git add <file>...\" to update what will be committed)\n");
    for f in &modified { baseline.push_str(&format!("\tmodified:   {}\n", f)); }
    baseline.push_str("\nUntracked files:\n");
    baseline.push_str("  (use \"git add <file>...\" to include in what will be committed)\n");
    for f in &untracked { baseline.push_str(&format!("\t{}\n", f)); }

    let mut porcelain = String::new();
    for f in &staged    { porcelain.push_str(&format!("M  {}\n", f)); }
    for f in &modified  { porcelain.push_str(&format!(" M {}\n", f)); }
    for f in &untracked { porcelain.push_str(&format!("?? {}\n", f)); }

    (baseline, porcelain)
}

/// `git log` — full verbose baseline + --oneline handler input, 25 commits.
fn git_log() -> (String /* verbose */, String /* oneline */) {
    let commits = [
        ("a1b2c3d", "feat: add user authentication middleware with JWT support"),
        ("e4f5g6h", "fix: resolve session token expiry edge case in auth service"),
        ("i7j8k9l", "refactor: extract database connection pool into separate module"),
        ("m0n1o2p", "feat: implement rate limiting for API endpoints"),
        ("q3r4s5t", "fix: correct pagination offset calculation in list endpoints"),
        ("u6v7w8x", "chore: update dependencies to latest stable versions"),
        ("y9z0a1b", "feat: add Redis cache layer for frequently accessed data"),
        ("c2d3e4f", "test: add integration tests for authentication flows"),
        ("g5h6i7j", "fix: handle null values in user profile update endpoint"),
        ("k8l9m0n", "feat: implement webhook delivery with exponential retry logic"),
        ("o1p2q3r", "refactor: consolidate error handling into shared middleware"),
        ("s4t5u6v", "fix: resolve race condition in concurrent request handler"),
        ("w7x8y9z", "feat: add audit logging for all sensitive data operations"),
        ("a0b1c2d", "chore: add GitHub Actions workflow for CI/CD pipeline"),
        ("e3f4g5h", "fix: correct CORS headers for cross-origin preflight requests"),
        ("i6j7k8l", "feat: implement file upload service with S3 integration"),
        ("m9n0o1p", "test: expand unit test coverage for all database models"),
        ("q2r3s4t", "fix: resolve memory leak in long-running background jobs"),
        ("u5v6w7x", "refactor: migrate all configuration to environment variables"),
        ("y8z9a0b", "feat: add Prometheus metrics endpoint for cluster monitoring"),
        ("c1d2e3f", "fix: handle graceful shutdown for all in-flight requests"),
        ("g4h5i6j", "docs: update API documentation with newly added endpoints"),
        ("k7l8m9n", "feat: implement automated database migration runner"),
        ("o0p1q2r", "fix: correct timestamp timezone handling in all API responses"),
        ("s3t4u5v", "chore: initial project setup with core dependency configuration"),
    ];
    let authors = [
        ("Alice Johnson", "alice@company.com"),
        ("Bob Smith",     "bob@company.com"),
        ("Carol White",   "carol@company.com"),
        ("David Brown",   "david@company.com"),
        ("Eve Martinez",  "eve@company.com"),
    ];
    let dates = [
        "Mon Mar 18 14:32:10 2024 +0000", "Fri Mar 15 10:15:42 2024 +0000",
        "Thu Mar 14 16:47:33 2024 +0000", "Wed Mar 13 09:22:18 2024 +0000",
        "Tue Mar 12 14:55:07 2024 +0000", "Mon Mar 11 11:30:59 2024 +0000",
        "Fri Mar  8 17:04:21 2024 +0000", "Thu Mar  7 13:18:44 2024 +0000",
        "Wed Mar  6 10:42:35 2024 +0000", "Tue Mar  5 15:29:16 2024 +0000",
        "Mon Mar  4 09:50:08 2024 +0000", "Fri Mar  1 16:37:52 2024 +0000",
        "Thu Feb 29 12:14:29 2024 +0000", "Wed Feb 28 09:45:11 2024 +0000",
        "Tue Feb 27 14:23:47 2024 +0000", "Mon Feb 26 11:06:33 2024 +0000",
        "Fri Feb 23 17:42:20 2024 +0000", "Thu Feb 22 13:55:04 2024 +0000",
        "Wed Feb 21 10:18:49 2024 +0000", "Tue Feb 20 15:01:37 2024 +0000",
        "Mon Feb 19 09:34:22 2024 +0000", "Fri Feb 16 16:47:15 2024 +0000",
        "Thu Feb 15 12:20:58 2024 +0000", "Wed Feb 14 09:03:41 2024 +0000",
        "Tue Feb 13 14:36:24 2024 +0000",
    ];

    let mut verbose = String::new();
    let mut oneline = String::new();
    for (i, (short_hash, msg)) in commits.iter().enumerate() {
        let (author, email) = authors[i % authors.len()];
        let date = dates[i];
        let full_hash = format!("{}abc123def456abc123def456abc123def456", short_hash);
        verbose.push_str(&format!(
            "commit {}\nAuthor: {} <{}>\nDate:   {}\n\n    {}\n\n",
            full_hash, author, email, date, msg
        ));
        oneline.push_str(&format!("{} {}\n", short_hash, msg));
    }
    (verbose, oneline)
}

/// `git diff` — five-file feature-branch diff with realistic 3-line context per hunk.
/// Real `git diff` uses -U3 (3 context lines before and after each change) by default.
/// The handler keeps structural lines + change lines + up to 2 context lines *after* a change;
/// it drops all context lines *before* a change. This becomes significant at scale.
fn git_diff() -> String {
    // Helper: build a realistic hunk with 3-line context before/after each change block.
    // Returns a String that looks exactly like `git diff -U3` output.
    fn hunk(before_start: u32, after_start: u32, context_before: &[&str],
            changes: &[(&str, &str)], context_after: &[&str]) -> String {
        // +/- lines interleaved
        let removed: Vec<&str> = changes.iter().map(|(r, _)| *r).filter(|s| !s.is_empty()).collect();
        let added:   Vec<&str> = changes.iter().map(|(_, a)| *a).filter(|s| !s.is_empty()).collect();
        let before_len = context_before.len() as u32 + removed.len() as u32 + context_after.len() as u32;
        let after_len  = context_before.len() as u32 + added.len()   as u32 + context_after.len() as u32;
        let mut s = format!("@@ -{},{} +{},{} @@\n", before_start, before_len, after_start, after_len);
        for c in context_before { s.push_str(&format!(" {}\n", c)); }
        for (rem, add) in changes {
            if !rem.is_empty() { s.push_str(&format!("-{}\n", rem)); }
            if !add.is_empty() { s.push_str(&format!("+{}\n", add)); }
        }
        for c in context_after { s.push_str(&format!(" {}\n", c)); }
        s
    }

    let mut out = String::new();

    // ── file 1: src/auth/middleware.ts ──────────────────────────────────────
    out.push_str("diff --git a/src/auth/middleware.ts b/src/auth/middleware.ts\n");
    out.push_str("index a1b2c3d..e4f5g6h 100644\n");
    out.push_str("--- a/src/auth/middleware.ts\n");
    out.push_str("+++ b/src/auth/middleware.ts\n");
    out.push_str(&hunk(1, 1,
        &["import { Request, Response, NextFunction } from 'express';",
          "import jwt from 'jsonwebtoken';",
          "import { config } from '../config';"],
        &[("", "import { logger } from '../utils/logger';"),
          ("", "import { AppError } from '../utils/errors';")],
        &["", "export function authenticate(req: Request, res: Response, next: NextFunction) {",
          "  const token = req.headers.authorization?.split(' ')[1];"],
    ));
    out.push_str(&hunk(10, 14,
        &["  const token = req.headers.authorization?.split(' ')[1];",
          "  if (!token) {",
          "    // no token"],
        &[("    return res.status(401).json({ error: 'No token provided' });",
           "    logger.warn('Request without authentication token', { path: req.path });"),
          ("", "    throw new AppError('Authentication required', 401);")],
        &["  }", "  try {", "    const decoded = jwt.verify(token, config.jwtSecret);"],
    ));
    out.push_str(&hunk(20, 26,
        &["    const decoded = jwt.verify(token, config.jwtSecret);",
          "    req.user = decoded as AuthUser;",
          "    // proceed"],
        &[("", "    logger.debug('Token verified', { userId: (decoded as AuthUser).id });")],
        &["    next();", "  } catch (error) {", "    // token invalid"],
    ));
    out.push_str(&hunk(27, 34,
        &["  } catch (error) {", "    // token invalid", "    // reject"],
        &[("    return res.status(401).json({ error: 'Invalid token' });",
           "    if (error instanceof jwt.TokenExpiredError) {"),
          ("", "      throw new AppError('Token has expired', 401);"),
          ("", "    }"),
          ("", "    logger.error('Token verification failed', { error });"),
          ("", "    throw new AppError('Invalid authentication token', 401);")],
        &["  }", "}"],
    ));

    // ── file 2: src/api/users.ts ────────────────────────────────────────────
    out.push_str("\ndiff --git a/src/api/users.ts b/src/api/users.ts\n");
    out.push_str("index b2c3d4e..f5g6h7i 100644\n");
    out.push_str("--- a/src/api/users.ts\n");
    out.push_str("+++ b/src/api/users.ts\n");
    out.push_str(&hunk(1, 1,
        &["import { Router } from 'express';",
          "import { UserService } from '../services/user';",
          "import { db } from '../db';"],
        &[("", "import { validateRequest } from '../middleware/validate';"),
          ("", "import { userSchema, updateUserSchema } from '../schemas/user';")],
        &["", "const router = Router();", "const userService = new UserService();"],
    ));
    out.push_str(&hunk(18, 22,
        &["router.get('/:id', async (req, res) => {",
          "  try {",
          "    const user = await userService.findById(req.params.id);"],
        &[("    res.json(user);",
           "    if (!user) { return res.status(404).json({ error: 'User not found' }); }"),
          ("", "    res.json({ data: user });")],
        &["  } catch (error) {",
          "    res.status(500).json({ error: 'Internal server error' });",
          "  }"],
    ));
    out.push_str(&hunk(30, 36,
        &["router.put('/:id', async (req, res) => {",
          "  try {",
          "    const user = await userService.update(req.params.id, req.body);"],
        &[("router.put('/:id', async (req, res) => {",
           "router.put('/:id', validateRequest(updateUserSchema), async (req, res, next) => {"),
          ("    const user = await userService.update(req.params.id, req.body);",
           "    const user = await userService.update(req.params.id, req.body, req.user);")],
        &["    if (!user) { return res.status(404).json({ error: 'User not found' }); }",
          "    res.json({ data: user });",
          "  } catch (error) {"],
    ));
    out.push_str(&hunk(38, 44,
        &["  } catch (error) {",
          "    res.status(500).json({ error: 'Internal server error' });",
          "  }"],
        &[("    res.status(500).json({ error: 'Internal server error' });", "    next(error);")],
        &["  }", "});", ""],
    ));

    // ── file 3: src/services/auth.ts ────────────────────────────────────────
    out.push_str("\ndiff --git a/src/services/auth.ts b/src/services/auth.ts\n");
    out.push_str("index c3d4e5f..g6h7i8j 100644\n");
    out.push_str("--- a/src/services/auth.ts\n");
    out.push_str("+++ b/src/services/auth.ts\n");
    out.push_str(&hunk(1, 1,
        &["import bcrypt from 'bcrypt';",
          "import jwt from 'jsonwebtoken';",
          "import { config } from '../config';"],
        &[("", "import { Redis } from 'ioredis';"),
          ("", "import { redisClient } from '../config/redis';")],
        &["", "export class AuthService {", "  private readonly saltRounds = 12;"],
    ));
    out.push_str(&hunk(22, 26,
        &["  async login(email: string, password: string): Promise<AuthResult> {",
          "    const user = await this.userRepository.findByEmail(email);",
          "    if (!user) {"],
        &[("      throw new Error('Invalid credentials');",
           "      throw new AppError('Invalid email or password', 401);")],
        &["    }", "    const isValid = await bcrypt.compare(password, user.passwordHash);",
          "    if (!isValid) {"],
    ));
    out.push_str(&hunk(28, 32,
        &["    const isValid = await bcrypt.compare(password, user.passwordHash);",
          "    if (!isValid) {",
          "      // wrong password"],
        &[("      throw new Error('Invalid credentials');",
           "      throw new AppError('Invalid email or password', 401);")],
        &["    }", "", "    // issue token"],
    ));
    out.push_str(&hunk(33, 38,
        &["    // issue token", "    // sign and return", ""],
        &[("    const token = jwt.sign({ userId: user.id }, config.jwtSecret, { expiresIn: '24h' });",
           "    const sessions = await redisClient.keys(`session:${user.id}:*`);"),
          ("", "    if (sessions.length > 0) { await redisClient.del(...sessions); }"),
          ("    return { token, user };",
           "    const token = jwt.sign({ userId: user.id, email: user.email }, config.jwtSecret, { expiresIn: '24h' });"),
          ("", "    const refresh = jwt.sign({ userId: user.id }, config.refreshSecret, { expiresIn: '7d' });"),
          ("", "    await redisClient.set(`session:${user.id}:${token}`, '1', 'EX', 86400);"),
          ("", "    return { token, refresh, user };")],
        &["  }", "}"],
    ));

    // ── file 4: src/models/user.ts ──────────────────────────────────────────
    out.push_str("\ndiff --git a/src/models/user.ts b/src/models/user.ts\n");
    out.push_str("index d4e5f6g..h7i8j9k 100644\n");
    out.push_str("--- a/src/models/user.ts\n");
    out.push_str("+++ b/src/models/user.ts\n");
    out.push_str(&hunk(1, 1,
        &["import { Entity, Column, PrimaryGeneratedColumn } from 'typeorm';",
          "import { IsEmail, IsString, MinLength } from 'class-validator';",
          ""],
        &[("", "import { Exclude } from 'class-transformer';")],
        &["@Entity('users')", "export class User {", "  @PrimaryGeneratedColumn('uuid')"],
    ));
    out.push_str(&hunk(15, 17,
        &["  @Column({ unique: true })",
          "  email: string;",
          ""],
        &[("  @Column()", "  @Column()"),
          ("  password: string;", "  @Exclude()"),
          ("", "  password: string;")],
        &["", "  @Column({ nullable: true })", "  refreshToken: string | null;"],
    ));
    out.push_str(&hunk(25, 28,
        &["  @Column({ default: false })",
          "  isEmailVerified: boolean;",
          ""],
        &[("", "  @Column({ type: 'timestamp', nullable: true })"),
          ("", "  lastLoginAt: Date | null;"),
          ("", "")],
        &["  @Column({ type: 'timestamp', default: () => 'CURRENT_TIMESTAMP' })",
          "  createdAt: Date;", ""],
    ));

    // ── file 5: src/config/database.ts ──────────────────────────────────────
    out.push_str("\ndiff --git a/src/config/database.ts b/src/config/database.ts\n");
    out.push_str("index e5f6g7h..i8j9k0l 100644\n");
    out.push_str("--- a/src/config/database.ts\n");
    out.push_str("+++ b/src/config/database.ts\n");
    out.push_str(&hunk(1, 1,
        &["import { DataSource } from 'typeorm';",
          "import { User } from '../models/user';",
          "import { Post } from '../models/post';"],
        &[("", "import { AuditLog } from '../models/audit-log';"),
          ("", "import { Session } from '../models/session';")],
        &["", "export const AppDataSource = new DataSource({", "  type: 'postgres',"],
    ));
    out.push_str(&hunk(10, 12,
        &["  entities: [User, Post],",
          "  synchronize: false,",
          "  logging: false,"],
        &[("  entities: [User, Post],", "  entities: [User, Post, AuditLog, Session],"),
          ("  logging: false,", "  logging: process.env.DB_LOGGING === 'true',")],
        &["  migrations: ['src/migrations/*.ts'],",
          "  subscribers: [],", "});"],
    ));

    out
}

/// Large git diff that hits the 200-line global cap.
///
/// Simulates a broad refactoring across 10 files: each file has 5 hunks with
/// 3 context lines before and after a small change block.  Total raw line count
/// exceeds the DIFF_TOTAL_CAP (200), so the cap truncation contributes to savings
/// on top of per-hunk context trimming.
fn git_diff_large() -> String {
    let mut out = String::new();

    let files = [
        "src/app.ts", "src/router.ts", "src/db.ts", "src/cache.ts",
        "src/logger.ts", "src/auth.ts", "src/users.ts", "src/posts.ts",
        "src/config.ts", "src/utils.ts",
    ];

    let ctx: [&str; 3] = [
        "  const result = await service.process(req.body);",
        "  if (!result) { throw new Error('Not found'); }",
        "  return res.json({ data: result, ok: true });",
    ];
    let after_ctx: [&str; 3] = [
        "  await logger.info('Request completed', { path: req.path });",
        "  metrics.increment('request.ok');",
        "  next();",
    ];

    for (fi, file) in files.iter().enumerate() {
        out.push_str(&format!("diff --git a/{f} b/{f}\n", f = file));
        out.push_str(&format!("index a1b2c{fi}..d4e5f{fi} 100644\n"));
        out.push_str(&format!("--- a/{f}\n+++ b/{f}\n", f = file));

        for hi in 0..5usize {
            let base = (fi * 50 + hi * 8 + 1) as u32;
            out.push_str(&format!("@@ -{},{} +{},{} @@ function handler_{}_{} {{\n",
                base, 8, base, 8, fi, hi));
            for c in &ctx {
                out.push_str(&format!(" {}\n", c));
            }
            out.push_str(&format!(
                "-  const old_val_{fi}_{hi} = config.get('legacy_key_{fi}_{hi}');\n"
            ));
            out.push_str(&format!(
                "+  const new_val_{fi}_{hi} = config.get('v2_key_{fi}_{hi}');\n"
            ));
            for c in &after_ctx {
                out.push_str(&format!(" {}\n", c));
            }
        }
    }
    out
}

/// `git push` — realistic object-counting noise.
fn git_push() -> String {
    concat!(
        "Enumerating objects: 147, done.\n",
        "Counting objects: 100% (147/147), done.\n",
        "Delta compression using up to 10 threads\n",
        "Compressing objects: 100% (89/89), done.\n",
        "Writing objects: 100% (98/98), 124.37 KiB | 4.16 MiB/s, done.\n",
        "Total 98 (delta 52), reused 0 (delta 0), pack-reused 0\n",
        "remote: Resolving deltas: 100% (52/52), completed with 31 local objects.\n",
        "To github.com:company/myapp.git\n",
        "   a1b2c3d..e4f5g6h  feature/user-auth -> feature/user-auth\n",
        "Branch 'feature/user-auth' set up to track remote branch 'feature/user-auth' from 'origin'.\n",
    ).to_string()
}

/// `ls -la` on a realistic large project root.
fn ls_project() -> String {
    concat!(
        "total 892\n",
        "drwxr-xr-x  28 user staff   896 Mar 18 14:32 .\n",
        "drwxr-xr-x  15 user staff   480 Mar 15 09:12 ..\n",
        "drwxr-xr-x  12 user staff   384 Mar 18 14:32 .git\n",
        "drwxr-xr-x   4 user staff   128 Mar 10 11:45 .github\n",
        "-rw-r--r--   1 user staff   543 Mar  5 16:20 .gitignore\n",
        "-rw-r--r--   1 user staff   892 Mar 12 10:30 .eslintrc.json\n",
        "-rw-r--r--   1 user staff   234 Mar  8 09:15 .prettierrc\n",
        "-rw-r--r--   1 user staff   128 Mar  5 16:20 .env.example\n",
        "drwxr-xr-x   4 user staff   128 Mar 18 14:00 .next\n",
        "drwxr-xr-x   3 user staff    96 Mar  5 16:20 .vscode\n",
        "-rw-r--r--   1 user staff  2341 Mar 15 11:20 Dockerfile\n",
        "-rw-r--r--   1 user staff   789 Mar 15 11:20 docker-compose.yml\n",
        "-rw-r--r--   1 user staff  1456 Mar 18 14:32 jest.config.ts\n",
        "-rw-r--r--   1 user staff  4521 Mar 16 10:45 package.json\n",
        "-rw-r--r--   1 user staff 89432 Mar 18 14:30 package-lock.json\n",
        "drwxr-xr-x 892 user staff 28544 Mar 18 14:30 node_modules\n",
        "-rw-r--r--   1 user staff   345 Mar 10 14:20 next.config.js\n",
        "-rw-r--r--   1 user staff  1234 Mar 14 09:30 tsconfig.json\n",
        "-rw-r--r--   1 user staff   567 Mar  5 16:20 README.md\n",
        "-rw-r--r--   1 user staff  2345 Mar 12 14:15 CONTRIBUTING.md\n",
        "drwxr-xr-x  18 user staff   576 Mar 18 14:32 src\n",
        "drwxr-xr-x   8 user staff   256 Mar 16 11:00 tests\n",
        "drwxr-xr-x   6 user staff   192 Mar 15 09:00 docs\n",
        "drwxr-xr-x   4 user staff   128 Mar 14 16:30 scripts\n",
        "drwxr-xr-x   3 user staff    96 Mar 12 10:00 migrations\n",
        "drwxr-xr-x   2 user staff    64 Mar 18 14:30 dist\n",
        "drwxr-xr-x   2 user staff    64 Mar 18 14:30 .cache\n",
        "-rw-r--r--   1 user staff   789 Mar 10 11:00 turbo.json\n",
    ).to_string()
}

/// `tsc` — 15 errors across 5 files in the compact `file(line,col): error TSxxxx: message` format
/// that tsc emits by default (no `--pretty` flag, which is common in CI and script invocations).
/// `tsc` — large monorepo with 120+ errors across 15 files, many repeated TS codes.
/// The handler deduplicates repeated codes per file (e.g. 12× TS2345 → one grouped line),
/// which produces the bulk of the savings.
fn tsc_errors() -> String {
    let mut out = String::new();

    // (file, line, code, message)
    // Each file has many repeated codes to trigger the deduplication path.
    let files: &[(&str, &[(&str, &str)])] = &[
        ("src/api/users.ts", &[
            ("12,5",  "TS2345: Argument of type 'string | undefined' is not assignable to parameter of type 'string'. Type 'undefined' is not assignable to type 'string'"),
            ("34,12", "TS2345: Argument of type 'number | undefined' is not assignable to parameter of type 'string'"),
            ("56,8",  "TS2345: Argument of type 'null' is not assignable to parameter of type 'string'"),
            ("78,3",  "TS2345: Argument of type 'unknown' is not assignable to parameter of type 'ApiUser'"),
            ("92,17", "TS2345: Argument of type 'string[]' is not assignable to parameter of type 'string'"),
            ("110,6", "TS2339: Property 'userId' does not exist on type 'Request'. Did you mean 'user'?"),
            ("134,9", "TS2339: Property 'email' does not exist on type '{}'"),
            ("156,4", "TS2339: Property 'role' does not exist on type 'JwtPayload'"),
            ("178,7", "TS7006: Parameter 'next' implicitly has an 'any' type"),
            ("201,2", "TS7006: Parameter 'req' implicitly has an 'any' type"),
            ("223,5", "TS7006: Parameter 'res' implicitly has an 'any' type"),
            ("245,1", "TS2322: Type 'null' is not assignable to type 'User'"),
        ]),
        ("src/api/products.ts", &[
            ("8,3",   "TS2345: Argument of type 'string | undefined' is not assignable to parameter of type 'string'. Type 'undefined' is not assignable to type 'string'"),
            ("19,7",  "TS2345: Argument of type 'number | undefined' is not assignable to parameter of type 'number'"),
            ("33,12", "TS2345: Argument of type 'Partial<Product>' is not assignable to parameter of type 'Product'"),
            ("47,5",  "TS7006: Parameter 'filter' implicitly has an 'any' type"),
            ("61,8",  "TS7006: Parameter 'options' implicitly has an 'any' type"),
            ("75,3",  "TS2322: Type 'string | null' is not assignable to type 'string'"),
            ("89,6",  "TS2322: Type 'undefined' is not assignable to type 'number'"),
            ("103,9", "TS2339: Property 'sku' does not exist on type 'ProductVariant'"),
        ]),
        ("src/api/orders.ts", &[
            ("15,4",  "TS2345: Argument of type 'string | undefined' is not assignable to parameter of type 'string'. Type 'undefined' is not assignable to type 'string'"),
            ("28,9",  "TS2345: Argument of type 'OrderStatus | null' is not assignable to parameter of type 'OrderStatus'"),
            ("42,6",  "TS2345: Argument of type 'number' is not assignable to parameter of type 'string'"),
            ("56,3",  "TS7006: Parameter 'ctx' implicitly has an 'any' type"),
            ("71,12", "TS7006: Parameter 'next' implicitly has an 'any' type"),
            ("85,5",  "TS2304: Cannot find name 'OrderService'"),
            ("99,8",  "TS2322: Type 'null' is not assignable to type 'Order'"),
        ]),
        ("src/auth/middleware.ts", &[
            ("11,3",  "TS7006: Parameter 'req' implicitly has an 'any' type"),
            ("22,7",  "TS7006: Parameter 'res' implicitly has an 'any' type"),
            ("33,5",  "TS7006: Parameter 'next' implicitly has an 'any' type"),
            ("44,9",  "TS2304: Cannot find name 'NextFunction'"),
            ("55,2",  "TS2304: Cannot find name 'Request'"),
            ("66,6",  "TS2304: Cannot find name 'Response'"),
            ("77,8",  "TS2345: Argument of type 'JwtPayload | string' is not assignable to parameter of type 'AuthUser'"),
            ("88,4",  "TS2345: Argument of type 'string | undefined' is not assignable to parameter of type 'string'. Type 'undefined' is not assignable to type 'string'"),
            ("99,11", "TS2339: Property 'user' does not exist on type 'Request'"),
        ]),
        ("src/auth/jwt.ts", &[
            ("9,5",   "TS2345: Argument of type 'string | undefined' is not assignable to parameter of type 'Secret'"),
            ("23,3",  "TS2345: Argument of type 'string | undefined' is not assignable to parameter of type 'string'. Type 'undefined' is not assignable to type 'string'"),
            ("37,8",  "TS2322: Type 'JwtPayload | string' is not assignable to type 'JwtPayload'"),
            ("51,6",  "TS2322: Type 'string | undefined' is not assignable to type 'string'"),
        ]),
        ("src/models/user.ts", &[
            ("7,1",   "TS1005: ',' expected"),
            ("18,4",  "TS1005: ';' expected"),
            ("29,7",  "TS2345: Argument of type 'number' is not assignable to parameter of type 'string'"),
            ("40,2",  "TS2345: Argument of type 'boolean | undefined' is not assignable to parameter of type 'boolean'"),
            ("51,9",  "TS2339: Property 'createdAt' does not exist on type 'UserInput'"),
            ("62,5",  "TS2339: Property 'updatedAt' does not exist on type 'UserInput'"),
        ]),
        ("src/components/UserCard.tsx", &[
            ("14,3",  "TS2741: Property 'onClick' is missing in type '{}' but required in type 'ButtonProps'"),
            ("28,7",  "TS2741: Property 'variant' is missing in type '{ size: string; }' but required in type 'ButtonProps'"),
            ("42,5",  "TS2322: Type 'string | null' is not assignable to type 'string'"),
            ("56,9",  "TS2322: Type 'number | undefined' is not assignable to type 'number'"),
            ("70,3",  "TS2339: Property 'loading' does not exist on type 'UserCardProps'"),
            ("84,6",  "TS2339: Property 'error' does not exist on type 'UserCardProps'"),
        ]),
        ("src/components/ProductGrid.tsx", &[
            ("11,4",  "TS2345: Argument of type 'Product[] | undefined' is not assignable to parameter of type 'Product[]'"),
            ("25,8",  "TS2345: Argument of type 'string | null' is not assignable to parameter of type 'string'"),
            ("39,5",  "TS2322: Type 'undefined' is not assignable to type 'ReactNode'"),
            ("53,7",  "TS7006: Parameter 'item' implicitly has an 'any' type"),
            ("67,3",  "TS7006: Parameter 'index' implicitly has an 'any' type"),
        ]),
        ("src/components/CheckoutForm.tsx", &[
            ("18,6",  "TS2345: Argument of type 'FormEvent<HTMLFormElement>' is not assignable to parameter of type 'SyntheticEvent'"),
            ("32,4",  "TS2345: Argument of type 'string | undefined' is not assignable to parameter of type 'string'. Type 'undefined' is not assignable to type 'string'"),
            ("46,9",  "TS2322: Type 'string' is not assignable to type 'number'"),
            ("60,2",  "TS2339: Property 'stripe' does not exist on type 'Window'"),
            ("74,7",  "TS2339: Property 'elements' does not exist on type 'StripeContext'"),
        ]),
        ("src/hooks/useAuth.ts", &[
            ("13,5",  "TS2345: Argument of type 'string | null' is not assignable to parameter of type 'string'"),
            ("27,8",  "TS2345: Argument of type 'AuthState | undefined' is not assignable to parameter of type 'AuthState'"),
            ("41,3",  "TS2322: Type 'null' is not assignable to type 'User'"),
            ("55,6",  "TS7006: Parameter 'dispatch' implicitly has an 'any' type"),
        ]),
        ("src/hooks/useProducts.ts", &[
            ("8,4",   "TS2345: Argument of type 'string | undefined' is not assignable to parameter of type 'string'. Type 'undefined' is not assignable to type 'string'"),
            ("22,7",  "TS2345: Argument of type 'Filter | null' is not assignable to parameter of type 'Filter'"),
            ("36,5",  "TS7006: Parameter 'query' implicitly has an 'any' type"),
            ("50,9",  "TS2339: Property 'total' does not exist on type 'ProductsResponse'"),
        ]),
        ("src/store/cartSlice.ts", &[
            ("16,3",  "TS2345: Argument of type 'string | undefined' is not assignable to parameter of type 'string'. Type 'undefined' is not assignable to type 'string'"),
            ("30,6",  "TS2322: Type 'CartItem | undefined' is not assignable to type 'CartItem'"),
            ("44,8",  "TS2339: Property 'quantity' does not exist on type 'never'"),
            ("58,4",  "TS2304: Cannot find name 'Draft'"),
        ]),
        ("src/lib/stripe.ts", &[
            ("12,7",  "TS2345: Argument of type 'string | undefined' is not assignable to parameter of type 'string'. Type 'undefined' is not assignable to type 'string'"),
            ("26,3",  "TS2322: Type 'Stripe | null' is not assignable to type 'Stripe'"),
            ("40,5",  "TS2345: Argument of type 'PaymentIntent | null' is not assignable to parameter of type 'PaymentIntent'"),
        ]),
        ("src/pages/checkout.tsx", &[
            ("9,6",   "TS2345: Argument of type 'string | undefined' is not assignable to parameter of type 'string'. Type 'undefined' is not assignable to type 'string'"),
            ("23,4",  "TS2345: Argument of type 'GetServerSidePropsContext | undefined' is not assignable to parameter of type 'GetServerSidePropsContext'"),
            ("37,8",  "TS2322: Type 'string | string[]' is not assignable to type 'string'"),
            ("51,2",  "TS2304: Cannot find name 'GetServerSideProps'"),
        ]),
        ("src/pages/profile.tsx", &[
            ("7,5",   "TS2345: Argument of type 'string | undefined' is not assignable to parameter of type 'string'. Type 'undefined' is not assignable to type 'string'"),
            ("21,9",  "TS2339: Property 'id' does not exist on type 'never'"),
            ("35,3",  "TS2304: Cannot find name 'useParams'"),
        ]),
    ];

    for (file, errors) in files {
        for (loc, msg) in *errors {
            out.push_str(&format!("{}({}): error {}\n", file, loc, msg));
        }
    }

    let total: usize = files.iter().map(|(_, e)| e.len()).sum();
    out.push_str(&format!("\nFound {} errors in {} files.\n", total, files.len()));
    out
}

/// `jest` — 10 test suites (2 failing), 150 tests total, 2 failures.
fn jest_output() -> String {
    let mut out = String::new();
    out.push_str(" PASS  tests/utils/format.test.ts (1.234 s)\n");
    out.push_str(" PASS  tests/utils/validate.test.ts (0.892 s)\n");
    out.push_str(" PASS  tests/models/user.test.ts (2.156 s)\n");
    out.push_str(" FAIL  tests/auth/jwt.test.ts (3.421 s)\n");
    out.push_str(" PASS  tests/api/health.test.ts (0.567 s)\n");
    out.push_str(" PASS  tests/api/users.test.ts (4.123 s)\n");
    out.push_str(" FAIL  tests/components/UserCard.test.tsx (2.789 s)\n");
    out.push_str(" PASS  tests/pages/Profile.test.tsx (1.456 s)\n");
    out.push_str(" PASS  tests/hooks/useAuth.test.ts (0.723 s)\n");
    out.push_str(" PASS  tests/services/auth.test.ts (3.234 s)\n");
    out.push_str("\n  ● auth/jwt › should reject expired tokens\n\n");
    out.push_str("    expect(received).toBe(expected)\n\n");
    out.push_str("    Expected: false\n");
    out.push_str("    Received: true\n\n");
    out.push_str("      at Object.<anonymous> (tests/auth/jwt.test.ts:47:5)\n");
    out.push_str("      at Promise.resolve.then (node_modules/jest-jasmine2/build/queueRunner.js:45:12)\n\n");
    out.push_str("  ● components/UserCard › renders user avatar correctly\n\n");
    out.push_str("    TestingLibraryElementError: Unable to find an accessible element with role \"img\"\n\n");
    out.push_str("      at getByRole (node_modules/@testing-library/dom/dist/queries/role.js:108:19)\n");
    out.push_str("      at Object.<anonymous> (tests/components/UserCard.test.tsx:34:26)\n\n");
    out.push_str("Test Suites: 2 failed, 8 passed, 10 total\n");
    out.push_str("Tests:       2 failed, 148 passed, 150 total\n");
    out.push_str("Snapshots:   0 total\n");
    out.push_str("Time:        20.595 s\n");
    out.push_str("Ran all test suites.\n");
    out
}

/// `pytest` — 200 tests (198 PASS, 2 FAIL) with platform header and PASSED lines.
fn pytest_output() -> String {
    let mut out = String::new();
    out.push_str("============================= test session info ==============================\n");
    out.push_str("platform linux -- Python 3.11.4, pytest-7.4.0, pluggy-1.0.0\n");
    out.push_str("rootdir: /home/user/project, configfile: pyproject.toml\n");
    out.push_str("plugins: anyio-3.7.0, cov-4.1.0, mock-3.11.1, asyncio-0.21.0\n");
    out.push_str("collected 200 items\n\n");
    // 198 passing tests
    let modules = [
        "tests/test_api.py", "tests/test_auth.py", "tests/test_models.py",
        "tests/test_services.py", "tests/test_utils.py", "tests/test_db.py",
        "tests/test_cache.py", "tests/test_events.py",
    ];
    for (i, &module) in modules.iter().cycle().take(198).enumerate() {
        out.push_str(&format!("PASSED {}::test_case_{} (0.{:02}s)\n", module, i + 1, (i % 99) + 1));
    }
    // 2 failures
    out.push_str("FAILED tests/test_api.py::test_create_user_duplicate_email\n");
    out.push_str("FAILED tests/test_auth.py::test_refresh_token_expired\n");
    out.push_str("\n_________________________________ test_create_user_duplicate_email _________________________________\n\n");
    out.push_str("    def test_create_user_duplicate_email():\n");
    out.push_str("        user = create_user(email=\"test@example.com\")\n");
    out.push_str(">       create_user(email=\"test@example.com\")\n\n");
    out.push_str("E       IntegrityError: (psycopg2.errors.UniqueViolation) duplicate key value violates unique constraint\n");
    out.push_str("E       DETAIL:  Key (email)=(test@example.com) already exists.\n\n");
    out.push_str("tests/test_api.py:45: IntegrityError\n");
    out.push_str("============================== short test summary info ===============================\n");
    out.push_str("FAILED tests/test_api.py::test_create_user_duplicate_email - IntegrityError\n");
    out.push_str("FAILED tests/test_auth.py::test_refresh_token_expired - AssertionError\n");
    out.push_str("========================= 2 failed, 198 passed in 12.34s ==========================\n");
    out
}

/// `vitest` — verbose output with 80 passing tests + 2 failing.
fn vitest_output() -> String {
    let mut out = String::new();
    out.push_str(" DEV  v1.2.0 /home/user/project\n\n");
    out.push_str(" ✓ src/utils/format.test.ts (12 tests) 45ms\n");
    out.push_str(" ✓ src/utils/validate.test.ts (8 tests) 32ms\n");
    out.push_str(" ✓ src/models/user.test.ts (15 tests) 78ms\n");
    out.push_str(" ✓ src/api/health.test.ts (5 tests) 23ms\n");
    out.push_str(" ✓ src/api/users.test.ts (20 tests) 156ms\n");
    out.push_str(" ✓ src/hooks/useAuth.test.ts (10 tests) 67ms\n");
    out.push_str(" ✓ src/services/auth.test.ts (8 tests) 89ms\n");
    for i in 1..=40 {
        out.push_str(&format!("   ✓ test case {} ({}ms)\n", i, i * 2 + 10));
    }
    out.push_str(" FAIL src/auth/jwt.test.ts\n");
    out.push_str("   × should reject expired tokens\n");
    out.push_str("     AssertionError: expected false to be true\n");
    out.push_str("       at /home/user/project/src/auth/jwt.test.ts:47:5\n\n");
    out.push_str(" FAIL src/components/UserCard.test.tsx\n");
    out.push_str("   × renders user avatar correctly\n");
    out.push_str("     TestingLibraryElementError: Unable to find an element with role \"img\"\n");
    out.push_str("       at /home/user/project/src/components/UserCard.test.tsx:34:26\n\n");
    out.push_str("Tests  2 failed | 80 passed (82)\n");
    out.push_str("Duration  2.34s\n");
    out
}

/// `eslint` — realistic large monorepo scan: 40 files with errors, ~200 total problems.
fn eslint_output() -> String {
    let mut out = String::new();

    // Realistic set of eslint rules that repeat across a codebase
    let errors: &[(&str, &str, &str)] = &[
        ("12:1",  "error",   "'React' must be in scope when using JSX  react/react-in-jsx-scope"),
        ("18:3",  "error",   "Unexpected any. Specify a different type  @typescript-eslint/no-explicit-any"),
        ("23:14", "warning", "'useEffect' is defined but never used  no-unused-vars"),
        ("31:7",  "error",   "Missing return type on function  @typescript-eslint/explicit-function-return-type"),
        ("45:22", "error",   "'Promise' not found in global scope  no-undef"),
        ("52:5",  "error",   "Unexpected use of 'console'  no-console"),
        ("67:12", "warning", "Expected a function body  arrow-body-style"),
        ("78:19", "error",   "Require statement not part of import statement  @typescript-eslint/no-var-requires"),
        ("89:3",  "error",   "img elements must have an alt prop  jsx-a11y/alt-text"),
        ("95:8",  "warning", "Do not pass children as props  react/no-children-prop"),
        ("103:4", "error",   "Do not use 'new' for side-effects  no-new"),
        ("115:9", "error",   "Promises must be awaited  @typescript-eslint/no-floating-promises"),
        ("128:6", "warning", "Unexpected var, use let or const instead  no-var"),
        ("134:1", "error",   "Parsing error: Unexpected token '?'"),
        ("142:7", "error",   "'data' is assigned a value but never used  no-unused-vars"),
    ];

    let files = [
        "src/api/users.ts",
        "src/api/products.ts",
        "src/api/orders.ts",
        "src/api/auth.ts",
        "src/api/payments.ts",
        "src/api/webhooks.ts",
        "src/auth/middleware.ts",
        "src/auth/jwt.ts",
        "src/auth/session.ts",
        "src/auth/oauth.ts",
        "src/components/UserCard.tsx",
        "src/components/ProductList.tsx",
        "src/components/OrderTable.tsx",
        "src/components/Dashboard.tsx",
        "src/components/Header.tsx",
        "src/components/Sidebar.tsx",
        "src/components/Modal.tsx",
        "src/components/forms/LoginForm.tsx",
        "src/components/forms/CheckoutForm.tsx",
        "src/components/forms/ProfileForm.tsx",
        "src/hooks/useAuth.ts",
        "src/hooks/useProducts.ts",
        "src/hooks/useOrders.ts",
        "src/hooks/useDebounce.ts",
        "src/hooks/usePagination.ts",
        "src/lib/db.ts",
        "src/lib/redis.ts",
        "src/lib/stripe.ts",
        "src/lib/email.ts",
        "src/lib/logger.ts",
        "src/store/authSlice.ts",
        "src/store/cartSlice.ts",
        "src/store/productSlice.ts",
        "src/store/orderSlice.ts",
        "src/utils/format.ts",
        "src/utils/validation.ts",
        "src/utils/constants.ts",
        "src/pages/index.tsx",
        "src/pages/checkout.tsx",
        "src/pages/profile.tsx",
    ];

    let mut total_errors = 0usize;
    let mut total_warnings = 0usize;

    for (i, file) in files.iter().enumerate() {
        // Each file gets a rotating subset of errors (3-6 per file)
        let start = i % errors.len();
        let count = 3 + (i % 4);
        out.push_str(file);
        out.push('\n');
        for j in 0..count {
            let (pos, sev, msg) = errors[(start + j) % errors.len()];
            out.push_str(&format!("  {}  {}  {}\n", pos, sev, msg));
            if sev == "error" { total_errors += 1; } else { total_warnings += 1; }
        }
        out.push_str(&format!("  ✖ {} problems\n", count));
        out.push('\n');
    }

    out.push_str(&format!(
        "✖ {} problems ({} errors, {} warnings)\n",
        total_errors + total_warnings, total_errors, total_warnings
    ));
    out
}

/// `npm install` — verbose with many WARN/notice lines.
fn npm_install_output() -> String {
    let mut out = String::new();
    out.push_str("npm warn deprecated rimraf@2.7.1: Rimraf versions prior to v4 are no longer supported\n");
    out.push_str("npm warn deprecated uuid@3.4.0: Please upgrade  to version 7 or higher.\n");
    out.push_str("npm warn deprecated glob@7.2.3: Glob versions prior to v9 are no longer supported\n");
    for i in 1..=20 {
        out.push_str(&format!("npm notice {}: created a lockfile as package-lock.json. You should commit this file.\n", i));
    }
    for pkg in &["lodash", "express", "axios", "react", "react-dom", "typescript", "webpack", "babel-core", "eslint", "jest"] {
        out.push_str(&format!("npm warn deprecated {}@legacy: Use latest version\n", pkg));
    }
    out.push_str("\nadded 847 packages from 623 contributors and audited 852 packages in 45.321s\n");
    out.push_str("\n94 packages are looking for funding\n");
    out.push_str("  run `npm fund` for details\n\n");
    out.push_str("found 3 vulnerabilities (1 low, 2 moderate)\n");
    out.push_str("  run `npm audit fix` to fix them, or `npm audit` for details\n");
    out
}

/// `kubectl get pods` — 50-pod table.
fn kubectl_pods() -> String {
    let mut out = String::new();
    out.push_str("NAME                                      READY   STATUS    RESTARTS   AGE     IP            NODE          NOMINATED NODE   READINESS GATES\n");
    let statuses = ["Running", "Running", "Running", "Pending", "CrashLoopBackOff"];
    for i in 0..50usize {
        let status = statuses[i % statuses.len()];
        let ready = if status == "Running" { "1/1" } else { "0/1" };
        let restarts = (i % 5) as u32;
        out.push_str(&format!(
            "api-deployment-{:016x}     {}     {}      {}          {}m   10.0.{}.{}   node-{}   <none>           <none>\n",
            i, ready, status, restarts, i + 1, i / 256, i % 256, i % 5
        ));
    }
    out
}

/// `terraform plan` — verbose plan output.  Handler keeps only + / - / ~ lines and "Plan:".
fn terraform_plan() -> String {
    let mut out = String::new();
    out.push_str("Refreshing Terraform state in-memory prior to plan...\n");
    out.push_str("The refreshed state will be used to calculate this plan, but will not be\n");
    out.push_str("persisted to local or remote state storage.\n\n");
    out.push_str("------------------------------------------------------------------------\n\n");
    out.push_str("An execution plan has been generated and is shown below.\n");
    out.push_str("Resource actions are indicated with the following symbols:\n");
    out.push_str("  + create\n  ~ update in-place\n  - destroy\n\n");
    out.push_str("Terraform will perform the following actions:\n\n");
    let resources = [
        ("aws_instance.web", "+"),
        ("aws_security_group.web", "+"),
        ("aws_s3_bucket.assets", "~"),
        ("aws_rds_instance.db", "~"),
        ("aws_lambda_function.processor", "+"),
        ("aws_iam_role.lambda", "+"),
        ("aws_cloudwatch_log_group.app", "+"),
        ("aws_sns_topic.alerts", "+"),
        ("aws_sqs_queue.tasks", "~"),
        ("aws_elasticache_cluster.cache", "-"),
    ];
    for (resource, symbol) in &resources {
        out.push_str(&format!("  # {} will be created/modified/destroyed\n", resource));
        out.push_str(&format!("  {} resource \"{}\" \"{}\" {{\n", symbol, resource.split('.').next().unwrap_or(""), resource.split('.').nth(1).unwrap_or("")));
        // Verbose attribute lines that get dropped
        out.push_str("      ami                          = \"ami-0c55b159cbfafe1f0\"\n");
        out.push_str("      arn                          = (known after apply)\n");
        out.push_str("      associate_public_ip_address  = (known after apply)\n");
        out.push_str("      availability_zone            = (known after apply)\n");
        out.push_str("      cpu_core_count               = (known after apply)\n");
        out.push_str("      cpu_threads_per_core         = (known after apply)\n");
        out.push_str("      disable_api_termination      = (known after apply)\n");
        out.push_str("      ebs_optimized                = (known after apply)\n");
        out.push_str("      get_password_data            = false\n");
        out.push_str("      hibernation                  = false\n");
        out.push_str("      host_id                      = (known after apply)\n");
        out.push_str("      id                           = (known after apply)\n");
        out.push_str("      instance_state               = (known after apply)\n");
        out.push_str("      instance_type                = \"t3.medium\"\n");
        out.push_str("      ipv6_address_count           = (known after apply)\n");
        out.push_str("      ipv6_addresses               = (known after apply)\n");
        out.push_str("      key_name                     = (known after apply)\n");
        out.push_str("      monitoring                   = (known after apply)\n");
        out.push_str("      outpost_arn                  = (known after apply)\n");
        out.push_str("      password_data                = (known after apply)\n");
        out.push_str("      placement_group              = (known after apply)\n");
        out.push_str("      primary_network_interface_id = (known after apply)\n");
        out.push_str("      private_dns                  = (known after apply)\n");
        out.push_str("      private_ip                   = (known after apply)\n");
        out.push_str("      public_dns                   = (known after apply)\n");
        out.push_str("      public_ip                    = (known after apply)\n");
        out.push_str("      security_groups              = (known after apply)\n");
        out.push_str("      source_dest_check            = true\n");
        out.push_str("      subnet_id                    = (known after apply)\n");
        out.push_str("      tags_all                     = (known after apply)\n");
        out.push_str("      tenancy                      = (known after apply)\n");
        out.push_str("      user_data                    = (known after apply)\n");
        out.push_str("      vpc_security_group_ids       = (known after apply)\n");
        out.push_str("    }\n\n");
    }
    out.push_str("Plan: 7 to add, 2 to change, 1 to destroy.\n\n");
    out.push_str("------------------------------------------------------------------------\n");
    out.push_str("Note: You didn't specify an \"-out\" parameter to save this plan, so Terraform\n");
    out.push_str("can't guarantee that exactly these actions will be performed if\n");
    out.push_str("\"terraform apply\" is subsequently run.\n");
    out
}

/// `docker ps -a` — many containers with full columns.
fn docker_ps_output() -> String {
    let mut out = String::new();
    out.push_str("CONTAINER ID   IMAGE                        COMMAND                  CREATED          STATUS                    PORTS                               NAMES\n");
    let images = ["nginx:1.24", "postgres:15", "redis:7", "node:20", "python:3.11", "golang:1.21", "rabbitmq:3", "elasticsearch:8"];
    let statuses = ["Up 3 hours", "Up 2 days", "Up 5 hours", "Exited (0) 1 hour ago", "Up 1 day"];
    let ports_list = ["0.0.0.0:80->80/tcp", "0.0.0.0:5432->5432/tcp", "6379/tcp", "", "0.0.0.0:3000->3000/tcp"];
    for i in 0..25usize {
        let cid = format!("{:012x}", i as u64 + 0xabc123000000u64);
        let image = images[i % images.len()];
        let status = statuses[i % statuses.len()];
        let ports = ports_list[i % ports_list.len()];
        out.push_str(&format!(
            "{}   {:<28}   \"/entrypoint.sh\"         2 days ago       {:<25}   {:<35}   service-{}\n",
            cid, image, status, ports, i + 1
        ));
    }
    out
}

/// `make build` — verbose with many make[N]: internals.
fn make_build_output() -> String {
    let mut out = String::new();
    let dirs = ["/project", "/project/src", "/project/lib", "/project/tools"];
    for dir in &dirs {
        out.push_str(&format!("make[1]: Entering directory '{}'\n", dir));
        out.push_str("gcc -Wall -O2 -c main.c -o main.o\n");
        out.push_str("gcc -Wall -O2 -c util.c -o util.o\n");
        out.push_str("gcc -Wall -O2 -c handler.c -o handler.o\n");
        out.push_str("ar rcs libutil.a util.o handler.o\n");
        out.push_str(&format!("make[1]: Leaving directory '{}'\n", dir));
        out.push_str(&format!("make[2]: Entering directory '{}'\n", dir));
        out.push_str("gcc -Wall -O2 -c config.c -o config.o\n");
        out.push_str("gcc -Wall -O2 -c logger.c -o logger.o\n");
        out.push_str(&format!("make[2]: Leaving directory '{}'\n", dir));
    }
    out.push_str("gcc -o myapp main.o util.o handler.o config.o logger.o -lm -lpthread\n");
    out.push_str("strip myapp\n");
    out.push_str("install -m 755 myapp /usr/local/bin/\n");
    out
}

/// `gh pr list` — tab-separated output for 20 PRs.
fn gh_pr_list_output() -> String {
    let mut out = String::new();
    let titles = [
        "feat: add OAuth2 support", "fix: resolve race condition in worker pool",
        "refactor: extract service layer from controllers", "docs: update API reference",
        "chore: upgrade dependencies to latest versions", "feat: implement rate limiting middleware",
        "fix: memory leak in connection pool", "test: add integration tests for auth flow",
        "feat: add WebSocket support", "fix: SQL injection in search endpoint",
        "refactor: migrate from REST to GraphQL", "chore: remove deprecated API v1 endpoints",
        "feat: add Prometheus metrics endpoint", "fix: incorrect timezone handling in scheduler",
        "docs: add architecture decision records", "feat: implement caching layer with Redis",
        "fix: CORS headers missing on preflight", "test: add load tests with k6",
        "feat: add OpenTelemetry tracing", "chore: update CI/CD pipeline config",
    ];
    for (i, title) in titles.iter().enumerate() {
        let num = i + 100;
        let state = if i % 5 == 0 { "DRAFT" } else { "OPEN" };
        let author = format!("dev{}", i % 4 + 1);
        let branch = title.replace(": ", "-").replace(' ', "-").to_lowercase();
        let branch = &branch[..branch.len().min(40)];
        out.push_str(&format!("{}\t{}\t{}\t@{}\t{}\t2024-01-{:02}T10:00:00Z\n", num, title, state, author, branch, i + 1));
    }
    out
}

/// `grep -rn` — many matches across many files.
fn grep_many_matches() -> String {
    let mut out = String::new();
    let files = [
        "src/api/users.rs", "src/api/auth.rs", "src/models/user.rs",
        "src/services/email.rs", "src/handlers/request.rs", "src/middleware/auth.rs",
        "src/config/database.rs", "src/utils/crypto.rs", "tests/integration.rs",
        "tests/unit/auth.rs",
    ];
    for (fi, file) in files.iter().enumerate() {
        for i in 0..15usize {
            let line_no = (fi * 30 + i * 2 + 1) as u32;
            let snippets = [
                "fn handle_request(req: &Request) -> Response {",
                "let token = extract_token(&req.headers)?;",
                "if !validate_token(&token) { return Err(AuthError::Invalid); }",
                "let user = db.find_user_by_token(&token).await?;",
                "log::debug!(\"Processing request for user {}\", user.id);",
            ];
            out.push_str(&format!("{}:{}:{}\n", file, line_no, snippets[i % snippets.len()]));
        }
    }
    out
}

/// `brew install` — verbose with download/pouring progress.
fn brew_install_output() -> String {
    let mut out = String::new();
    out.push_str("==> Downloading https://formulae.brew.sh/api/formula.jws.json\n");
    out.push_str("############################################ 100.0%\n");
    out.push_str("==> Downloading https://formulae.brew.sh/api/cask.jws.json\n");
    out.push_str("############################################ 100.0%\n");
    out.push_str("==> Fetching dependencies for ripgrep: pcre2\n");
    out.push_str("==> Downloading https://ghcr.io/v2/homebrew/core/pcre2/manifests/10.42-1\n");
    out.push_str("############################################ 100.0%\n");
    out.push_str("==> Downloading https://ghcr.io/v2/homebrew/core/pcre2/blobs/sha256:abc123\n");
    out.push_str("############################################ 100.0%\n");
    out.push_str("==> Downloading https://ghcr.io/v2/homebrew/core/ripgrep/manifests/13.0.0-1\n");
    out.push_str("############################################ 100.0%\n");
    out.push_str("==> Downloading https://ghcr.io/v2/homebrew/core/ripgrep/blobs/sha256:def456\n");
    out.push_str("############################################ 100.0%\n");
    out.push_str("==> Installing dependencies for ripgrep: pcre2\n");
    out.push_str("==> Installing ripgrep dependency: pcre2\n");
    out.push_str("==> Pouring pcre2--10.42.arm64_ventura.bottle.tar.gz\n");
    out.push_str("🍺  /opt/homebrew/Cellar/pcre2/10.42: 230 files, 6.5MB\n");
    out.push_str("==> Installing ripgrep\n");
    out.push_str("==> Pouring ripgrep--13.0.0.arm64_ventura.bottle.tar.gz\n");
    out.push_str("==> Caveats\n");
    out.push_str("Bash completion has been installed to: /opt/homebrew/etc/bash_completion.d\n");
    out.push_str("==> Summary\n");
    out.push_str("🍺  /opt/homebrew/Cellar/ripgrep/13.0.0: 14 files, 3.8MB\n");
    out.push_str("==> Running `brew cleanup ripgrep`...\n");
    out.push_str("Disable this behaviour by setting HOMEBREW_NO_INSTALL_CLEANUP.\n");
    out
}

/// `ember build` — realistic Ember CLI build with fingerprint spam + 6 errors.
/// Progress/fingerprint lines (~140) are dropped; only errors + summary are kept.
fn ember_build_output() -> String {
    let mut out = String::new();
    out.push_str("Build failed.\n");
    out.push_str("cleaning up...\n\n");
    // Fingerprint / asset pipeline noise (dropped by handler)
    let asset_names = [
        "app", "vendor", "test-support", "test-loader",
        "ember-fetch/fetch", "@ember/test-helpers/index",
    ];
    for chunk in &["css", "js", "map"] {
        for name in &asset_names {
            for i in 0..12usize {
                out.push_str(&format!(
                    "  {} {}-{:x}{:x}.{}\n",
                    name, name, i * 0xdeadbeef % 0xffff, i * 0xcafe % 0xffff, chunk
                ));
            }
        }
    }
    // Build progress lines (dropped)
    out.push_str("building: Compiling...\n");
    out.push_str("building: ConcatMerge\n");
    out.push_str("building: PrependTransform\n");
    for module in &[
        "app/router.js", "app/app.js", "app/index.html",
        "app/styles/app.css", "config/environment.js",
    ] {
        out.push_str(&format!("  {}\n", module));
    }
    out.push_str("Babel plugins detected\n");
    out.push_str("annotation: Build\n");
    out.push_str("annotation: Concat: App\n");
    out.push_str("annotation: Concat: Vendor\n");
    // Actual errors (kept by handler)
    out.push_str("app/components/user-profile.hbs:8:7: Error: Unexpected token '<'\n");
    out.push_str("app/components/nav-bar.ts:14:3: error TS2345: Argument of type 'string' is not assignable to parameter of type 'number'.\n");
    out.push_str("app/templates/index.hbs:22:1: Error: Unclosed element `div`\n");
    out.push_str("app/components/modal.js:55:5: Error: 'user' is not defined\n");
    out.push_str("app/services/session.ts:33:1: error TS2304: Cannot find name 'inject'\n");
    out.push_str("app/helpers/format-date.ts:12:3: error TS2307: Cannot find module '@ember/component' or its corresponding type declarations.\n");
    out.push_str("\nBuild failed (3241ms)\n");
    out
}

/// `go test ./...` — 150 tests with === RUN / --- PASS noise + 2 failures.
fn go_test_output() -> String {
    let mut out = String::new();
    let packages = [
        "github.com/company/project/api",
        "github.com/company/project/auth",
        "github.com/company/project/models",
        "github.com/company/project/services",
        "github.com/company/project/utils",
    ];
    for (pi, pkg) in packages.iter().enumerate() {
        for i in 0..28usize {
            out.push_str(&format!("=== RUN   TestCase_{:03}\n", pi * 30 + i));
            out.push_str(&format!("=== PAUSE TestCase_{:03}\n", pi * 30 + i));
            out.push_str(&format!("=== CONT  TestCase_{:03}\n", pi * 30 + i));
        }
        // 2 failures in the first package
        if pi == 0 {
            out.push_str("--- FAIL: TestCreateUser (0.15s)\n");
            out.push_str("    user_test.go:45: expected email to be unique, got duplicate\n");
            out.push_str("    user_test.go:46: want: nil, got: &DuplicateKeyError{}\n");
            out.push_str("--- FAIL: TestRefreshToken (0.08s)\n");
            out.push_str("    auth_test.go:89: token should be expired, validation returned true\n");
            out.push_str(&format!("FAIL\t{}\t0.31s\n", pkg));
        } else {
            for i in 0..28usize {
                out.push_str(&format!("--- PASS: TestCase_{:03} (0.{:02}s)\n", pi * 30 + i, i + 1));
            }
            out.push_str(&format!("ok  \t{}\t{}.{}s\n", pkg, pi, pi * 3 + 1));
        }
        out.push_str(&format!("coverage: {}.{}% of statements in {}\n", 75 + pi * 2, pi * 3, pkg));
    }
    out
}

/// `mvn install` — verbose Maven build with many [INFO] lines.
/// `mvn install` — realistic 8-module build with dependency downloads and verbose [INFO] noise.
/// The handler strips Downloading/Downloaded/Progress/Compiling/bare-[INFO] lines,
/// keeping only plugin separators, test results, and the reactor summary.
fn maven_output() -> String {
    let mut out = String::new();
    out.push_str("[INFO] Scanning for projects...\n");
    out.push_str("[INFO] ------------------------------------------------------------------------\n");
    out.push_str("[INFO] Reactor Build Order:\n");
    for m in &["my-parent", "my-common", "my-core", "my-data", "my-service", "my-api", "my-web", "my-integration-tests"] {
        out.push_str(&format!("[INFO]   {} [jar]\n", m));
    }
    out.push_str("[INFO] ------------------------------------------------------------------------\n\n");

    // Dependency resolution noise (all dropped by handler)
    let deps = [
        "org.springframework.boot:spring-boot-starter:3.2.1",
        "org.springframework.boot:spring-boot-autoconfigure:3.2.1",
        "org.springframework:spring-core:6.1.3",
        "org.springframework:spring-context:6.1.3",
        "org.springframework:spring-beans:6.1.3",
        "org.springframework:spring-aop:6.1.3",
        "org.springframework.data:spring-data-jpa:3.2.1",
        "org.hibernate.orm:hibernate-core:6.4.1",
        "jakarta.persistence:jakarta.persistence-api:3.1.0",
        "com.zaxxer:HikariCP:5.1.0",
        "org.postgresql:postgresql:42.7.1",
        "org.flywaydb:flyway-core:10.6.0",
        "io.jsonwebtoken:jjwt-api:0.12.3",
        "org.springdoc:springdoc-openapi-starter-webmvc-ui:2.3.0",
        "com.fasterxml.jackson.core:jackson-databind:2.16.1",
    ];
    for dep in &deps {
        out.push_str(&format!("[INFO] Downloading from central: https://repo.maven.apache.org/maven2/{}\n", dep.replace(':', "/")));
    }
    for dep in &deps {
        let kb = 45 + dep.len() % 200;
        out.push_str(&format!("[INFO] Downloaded from central: https://repo.maven.apache.org/maven2/{} ({} kB at 1.2 MB/s)\n", dep.replace(':', "/"), kb));
    }
    out.push_str("[INFO] Progress (1): Downloading 8/15 (53%)\n");
    out.push_str("[INFO] Progress (1): Downloading 15/15\n\n");

    let modules = [
        ("my-parent",             0.312,  0,  0),
        ("my-common",             3.891, 12,  0),
        ("my-core",              12.456, 87,  0),
        ("my-data",               8.234, 54,  0),
        ("my-service",           15.789, 134, 0),
        ("my-api",                6.123, 43,  0),
        ("my-web",                9.456, 67,  0),
        ("my-integration-tests",  4.678, 38,  0),
    ];

    for (module, elapsed, tests, failures) in &modules {
        out.push_str(&format!("[INFO] ------------------------------------------------------------------------\n"));
        out.push_str(&format!("[INFO] Building {} 1.0.0-SNAPSHOT\n", module));
        out.push_str(&format!("[INFO] --- maven-resources-plugin:3.3.0:resources (default-resources) @ {} ---\n", module));
        out.push_str("[INFO] Copying 15 resources from src/main/resources to target/classes\n");
        out.push_str("[INFO]\n");
        out.push_str(&format!("[INFO] --- maven-compiler-plugin:3.11.0:compile (default-compile) @ {} ---\n", module));
        out.push_str("[INFO] Changes detected - recompiling the module! :dependency\n");
        out.push_str("[INFO] Compiling 64 source files with javac [debug release 17] to target/classes\n");
        out.push_str("[INFO]\n");
        out.push_str(&format!("[INFO] --- maven-compiler-plugin:3.11.0:testCompile (default-testCompile) @ {} ---\n", module));
        out.push_str("[INFO] Compiling 23 source files with javac [debug release 17] to target/test-classes\n");
        out.push_str("[INFO]\n");
        if *tests > 0 {
            out.push_str(&format!("[INFO] --- maven-surefire-plugin:3.1.2:test (default-test) @ {} ---\n", module));
            out.push_str("[INFO] Using auto detected provider org.apache.maven.surefire.junit5.JUnit5Provider\n");
            out.push_str(&format!("[INFO] Tests run: {}, Failures: {}, Errors: 0, Skipped: 0, Time elapsed: {:.3} s\n", tests, failures, elapsed * 0.4));
            out.push_str("[INFO]\n");
        }
        out.push_str(&format!("[INFO] --- maven-jar-plugin:3.3.0:jar (default-jar) @ {} ---\n", module));
        out.push_str(&format!("[INFO] Building jar: /project/{}/target/{}-1.0.0-SNAPSHOT.jar\n", module, module));
        out.push_str("[INFO]\n");
        out.push_str(&format!("[INFO] --- maven-install-plugin:3.1.1:install (default-install) @ {} ---\n", module));
        out.push_str(&format!("[INFO] Installing /project/{}/target/{}-1.0.0-SNAPSHOT.jar to /home/user/.m2/repository/com/example/{}/1.0.0-SNAPSHOT/{}-1.0.0-SNAPSHOT.jar\n", module, module, module, module));
        out.push_str("[INFO]\n");
    }

    out.push_str("[INFO] ------------------------------------------------------------------------\n");
    out.push_str("[INFO] Reactor Summary for my-parent 1.0.0-SNAPSHOT:\n");
    for (module, elapsed, _, _) in &modules {
        out.push_str(&format!("[INFO]  * {:40} SUCCESS [ {:6.3} s]\n", module, elapsed));
    }
    out.push_str("[INFO] ------------------------------------------------------------------------\n");
    out.push_str("[INFO] BUILD SUCCESS\n");
    out.push_str("[INFO] ------------------------------------------------------------------------\n");
    out.push_str("[INFO] Total time:  60.939 s\n");
    out.push_str("[INFO] Finished at: 2024-01-15T10:23:45+00:00\n");
    out.push_str("[INFO] ------------------------------------------------------------------------\n");
    out
}

/// `env` — large environment dump with 100+ variables including sensitive ones.
fn env_large() -> String {
    let mut out = String::new();
    // PATH group
    out.push_str("PATH=/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin:/usr/games:/usr/local/games:/snap/bin:/home/user/.cargo/bin:/home/user/.local/bin\n");
    out.push_str("MANPATH=/usr/local/man:/usr/man:/usr/share/man\n");
    out.push_str("PYTHONPATH=/home/user/project/lib:/home/user/.local/lib/python3.11\n");
    // Sensitive (should be dropped)
    out.push_str("AWS_SECRET_ACCESS_KEY=wJalrXUtnFEMI/K7MDENG/bPxRfiCYEXAMPLEKEY\n");
    out.push_str("DATABASE_PASSWORD=super_secret_db_password_123\n");
    out.push_str("JWT_SECRET=my-very-long-jwt-secret-that-should-not-appear-in-output\n");
    out.push_str("GITHUB_TOKEN=ghp_xxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxx\n");
    out.push_str("API_KEY=sk-proj-xxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxx\n");
    // Lang/Runtime group
    out.push_str("PYTHON_VERSION=3.11.4\n");
    out.push_str("GOPATH=/home/user/go\n");
    out.push_str("GOROOT=/usr/local/go\n");
    out.push_str("GOVERSION=go1.21.0\n");
    out.push_str("NODE_ENV=development\n");
    out.push_str("NODE_PATH=/home/user/.nvm/versions/node/v20.0.0/lib/node_modules\n");
    out.push_str("RUST_LOG=info\n");
    out.push_str("CARGO_HOME=/home/user/.cargo\n");
    // Cloud/Services group
    out.push_str("AWS_REGION=us-east-1\n");
    out.push_str("AWS_ACCOUNT_ID=123456789012\n");
    out.push_str("DATABASE_URL=postgresql://localhost:5432/mydb\n");
    out.push_str("REDIS_HOST=localhost\n");
    out.push_str("REDIS_PORT=6379\n");
    out.push_str("MONGO_URI=mongodb://localhost:27017/mydb\n");
    // Tools group
    out.push_str("EDITOR=nvim\n");
    out.push_str("SHELL=/bin/zsh\n");
    out.push_str("TERM=xterm-256color\n");
    out.push_str("GIT_AUTHOR_NAME=John Doe\n");
    out.push_str("GIT_AUTHOR_EMAIL=john@example.com\n");
    out.push_str("DOCKER_HOST=unix:///var/run/docker.sock\n");
    out.push_str("KUBECONFIG=/home/user/.kube/config\n");
    // Other (many noise vars)
    for i in 1..=50usize {
        out.push_str(&format!("APP_CONFIG_VAR_{}=some_value_{}_for_application_configuration\n", i, i));
    }
    out.push_str("USER=john\n");
    out.push_str("HOME=/home/john\n");
    out.push_str("HOSTNAME=dev-machine-01\n");
    out.push_str("LANG=en_US.UTF-8\n");
    out.push_str("LC_ALL=en_US.UTF-8\n");
    out.push_str("COLORTERM=truecolor\n");
    out.push_str("DISPLAY=:0\n");
    out.push_str("DBUS_SESSION_BUS_ADDRESS=unix:path=/run/user/1000/bus\n");
    out.push_str("XDG_RUNTIME_DIR=/run/user/1000\n");
    out.push_str("XDG_SESSION_TYPE=x11\n");
    out.push_str("LOGNAME=john\n");
    out.push_str("SSH_AUTH_SOCK=/tmp/ssh-xxxxx/agent.1234\n");
    out.push_str("GPG_AGENT_INFO=/run/user/1000/gnupg/S.gpg-agent:0:1\n");
    out
}

/// `cargo clippy` — many warnings with full span output (-->, |, = note:, = help:).
fn clippy_verbose() -> String {
    let mut out = String::new();
    out.push_str("   Checking myapp v0.1.0 (/home/user/project)\n");
    let warnings = [
        ("unused variable `result`", "unused_variables", "src/main.rs", 12, "consider prefixing with an underscore: `_result`"),
        ("unused variable `config`", "unused_variables", "src/config.rs", 45, "consider prefixing with an underscore: `_config`"),
        ("function is never used: `helper`", "dead_code", "src/utils.rs", 8, ""),
        ("unused import: `std::collections::HashMap`", "unused_imports", "src/api.rs", 3, "remove the whole `use` item"),
        ("unnecessary clone of `Arc`", "clippy::arc_with_non_send_sync", "src/worker.rs", 67, ""),
        ("match arm can be simplified using `if let`", "clippy::redundant_pattern_matching", "src/handler.rs", 23, "use `if let` instead"),
        ("use of `unwrap()` on `Option` value", "clippy::unwrap_used", "src/db.rs", 89, ""),
        ("this expression creates a reference which is immediately dereferenced", "clippy::needless_borrow", "src/service.rs", 34, ""),
        ("redundant closure", "clippy::redundant_closure", "src/router.rs", 56, ""),
        ("this `if`-`else` expression can be collapsed", "clippy::collapsible_else_if", "src/auth.rs", 78, ""),
    ];
    for (msg, lint, file, line, help) in &warnings {
        out.push_str(&format!("warning: {} [{}]\n", msg, lint));
        out.push_str(&format!("  --> {}:{}:5\n", file, line));
        out.push_str(&format!("   |\n"));
        out.push_str(&format!("{}  |     let {} = compute_value();\n", line, "x"));
        out.push_str("   |     ^^^^^^^^^^^^^^^^^^^^\n");
        out.push_str("   |\n");
        if !help.is_empty() {
            out.push_str(&format!("   = help: {}\n", help));
        }
        out.push_str("   = note: `#[warn({})]` on by default\n");
        out.push('\n');
    }
    out.push_str("   Compiling myapp v0.1.0 (/home/user/project)\n");
    out.push_str("warning: 10 warnings emitted\n\n");
    out.push_str("    Finished `dev` profile [unoptimized + debuginfo] target(s) in 3.45s\n");
    out
}

/// `golangci-lint run` — many INFO/DEBUG lines + diagnostics.
/// `golangci-lint` — large project with heavy INFO/DEBU preamble and 100+ diagnostics.
/// The handler drops all INFO/DEBU lines and caps diagnostics at 40 + "[+N more]".
fn golangci_output() -> String {
    let mut out = String::new();

    // Heavy INFO/DEBU preamble (all dropped by handler)
    out.push_str("INFO [config] Config search paths: [/home/user/project /home/user]\n");
    out.push_str("INFO [config] Used config file /home/user/project/.golangci.yml\n");
    out.push_str("INFO [config] Run info: linters: 18, issues providers: 3\n");
    out.push_str("INFO [loader] Go packages loading in PACKAGES mode with GOFLAGS=\n");
    out.push_str("INFO [loader] Packages load duration: 2.891s\n");
    out.push_str("INFO [loader] Loaded 47 packages\n");
    out.push_str("INFO [runner] Processors count: 12\n");
    out.push_str("INFO [runner] Starting all linters...\n");
    for linter in &["errcheck", "gosimple", "govet", "ineffassign", "staticcheck",
                    "unused", "deadcode", "varcheck", "structcheck", "golint",
                    "revive", "gocyclo", "gofmt", "goimports", "misspell",
                    "godot", "godox", "nlreturn"] {
        out.push_str(&format!("INFO [runner] Running linter: {}\n", linter));
        out.push_str(&format!("DEBU [runner] linter {} took 0.{}s\n", linter, linter.len() * 37 % 999));
    }
    out.push_str("INFO [runner] All linters finished\n");
    out.push_str("INFO [runner] Processing 124 issues\n");
    out.push_str("INFO [runner] Sorting issues\n");
    out.push_str("WARN linters settings for 'structcheck' are not supported by golangci-lint v1.57+\n");
    out.push_str("WARN linters settings for 'varcheck' are not supported by golangci-lint v1.57+\n");
    out.push_str("WARN linters settings for 'deadcode' are not supported by golangci-lint v1.57+\n");

    // Diagnostics: 12 files × 10 issues = 120 diagnostics (handler keeps 40, shows [+80 more])
    let files = [
        "pkg/api/handler.go",
        "pkg/api/middleware.go",
        "pkg/auth/jwt.go",
        "pkg/auth/session.go",
        "pkg/models/user.go",
        "pkg/models/order.go",
        "pkg/models/product.go",
        "internal/db/postgres.go",
        "internal/db/migrations.go",
        "internal/cache/redis.go",
        "cmd/server/main.go",
        "cmd/worker/main.go",
    ];
    let issues: &[(&str, &str)] = &[
        ("12:9",  "ineffectual assignment to err (ineffassign)"),
        ("27:3",  "error return value not checked (errcheck)"),
        ("41:14", "S1000: use plain channel send or receive instead of select with a single case (gosimple)"),
        ("56:5",  "declared and not used: `ctx` (unused)"),
        ("70:12", "exported function `GetUser` should have comment or be unexported (golint)"),
        ("84:7",  "SA4006: this value of `err` is never used (staticcheck)"),
        ("98:3",  "SA1006: Printf with dynamic first argument and no further arguments (staticcheck)"),
        ("112:18","printf: fmt.Sprintf can be replaced with fmt.Sprint (govet)"),
        ("126:5", "Function 'ProcessOrder' has too high cyclomatic complexity (12 > 10) (gocyclo)"),
        ("140:9", "Comment should end with a period (godot)"),
    ];
    for file in &files {
        for (pos, issue) in issues {
            out.push_str(&format!("{}:{}:{}\n", file, pos, issue));
        }
    }

    out
}

/// `next build` — verbose output with route table and chunk manifests.
fn next_build_output() -> String {
    let mut out = String::new();
    out.push_str("info  - Loaded env from /home/user/project/.env.local\n");
    out.push_str("info  - Loaded env from /home/user/project/.env\n");
    out.push_str("   Creating an optimized production build ...\n");
    out.push_str("✓ Compiled successfully\n");
    out.push_str("✓ Linting and checking validity of types\n");
    out.push_str("✓ Collecting page data\n");
    out.push_str("   Generating static pages (0/48)\n");
    out.push_str("   Generating static pages (12/48)\n");
    out.push_str("   Generating static pages (24/48)\n");
    out.push_str("   Generating static pages (36/48)\n");
    out.push_str("✓ Generating static pages (48/48)\n");
    out.push_str("✓ Finalizing page optimization\n");
    out.push_str("✓ Collecting build traces\n");
    // Route table (├/└/│ lines get dropped by next handler)
    out.push_str("Route (app)                              Size     First Load JS\n");
    out.push_str("┌ ○ /                                   5.12 kB        96.4 kB\n");
    out.push_str("├ ○ /_not-found                         884 B          88.2 kB\n");
    out.push_str("├ ○ /about                              3.45 kB        94.7 kB\n");
    out.push_str("├ ƒ /api/auth/[...nextauth]             0 B            87.3 kB\n");
    out.push_str("├ ƒ /api/users                          0 B            87.3 kB\n");
    out.push_str("├ ○ /blog                               12.3 kB       103.6 kB\n");
    out.push_str("├ ● /blog/[slug]                        4.23 kB        95.6 kB\n");
    out.push_str("├   └ /blog/hello-world\n");
    out.push_str("├   └ /blog/getting-started\n");
    out.push_str("├   └ /blog/advanced-patterns\n");
    out.push_str("├ ○ /contact                            2.1 kB         93.4 kB\n");
    out.push_str("├ ƒ /dashboard                          8.9 kB        100.2 kB\n");
    out.push_str("├ ○ /docs                               15.6 kB       107.0 kB\n");
    out.push_str("├ ƒ /settings                           6.7 kB         97.9 kB\n");
    out.push_str("└ ○ /signup                             4.5 kB         95.8 kB\n");
    out.push_str("+ First Load JS shared by all           87.3 kB\n");
    out.push_str("  chunks/framework-aec844d2ccbe3c5c.js  45.2 kB\n");
    out.push_str("  chunks/main-app-f8c6e7d9a1b2c3d4.js   32.1 kB\n");
    out.push_str("  chunks/webpack-abc123def456.js          2.4 kB\n");
    out.push_str("  css/styles-12345678.css                7.6 kB\n");
    out.push_str("\n");
    out.push_str("○  (Static)   prerendered as static content\n");
    out.push_str("●  (SSG)      prerendered as static HTML (uses getStaticProps)\n");
    out.push_str("ƒ  (Dynamic)  server-rendered on demand\n");
    out.push_str("\nCompiled in 12.3s\n");
    out
}

/// `playwright test` — output with verbose browser setup + failures.
fn playwright_output() -> String {
    let mut out = String::new();
    out.push_str("Running 45 tests using 3 workers\n\n");
    // Passing tests
    for i in 0..40usize {
        let browsers = ["chromium", "firefox", "webkit"];
        let browser = browsers[i % 3];
        out.push_str(&format!("  ✓  {} › home page › displays navigation menu ({}ms)\n", browser, 200 + i * 10));
        out.push_str(&format!("  ✓  {} › auth › login with valid credentials ({}ms)\n", browser, 500 + i * 5));
    }
    // 2 failures
    out.push_str("  1) chromium › checkout › payment form › submits successfully ─────────\n\n");
    out.push_str("    Error: expect(received).toBeVisible()\n\n");
    out.push_str("    Expected: visible\n");
    out.push_str("    Received: <button data-testid=\"submit\"> is hidden\n\n");
    out.push_str("    at Object.<anonymous> (tests/checkout.spec.ts:89:34)\n\n");
    out.push_str("  2) firefox › checkout › payment form › shows validation errors ───────\n\n");
    out.push_str("    TimeoutError: locator.click: Timeout 30000ms exceeded.\n");
    out.push_str("    at Object.<anonymous> (tests/checkout.spec.ts:112:20)\n\n");
    out.push_str("  40 passed (45.3s)\n");
    out.push_str("  2 failed\n");
    out.push_str("  5 skipped\n");
    out
}

/// `pip install -r requirements.txt` — many Collecting/Downloading lines.
fn pip_install_output() -> String {
    let mut out = String::new();
    let packages = [
        "django", "djangorestframework", "celery", "redis", "psycopg2-binary",
        "boto3", "requests", "Pillow", "cryptography", "PyJWT",
        "sqlalchemy", "alembic", "pytest", "pytest-django", "factory-boy",
        "black", "isort", "mypy", "flake8", "coverage",
    ];
    for pkg in &packages {
        out.push_str(&format!("Collecting {}\n", pkg));
        out.push_str(&format!("  Downloading {}-5.0.0-py3-none-any.whl (2.1 MB)\n", pkg));
        out.push_str("     ━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━ 2.1/2.1 MB 8.5 MB/s eta 0:00:00\n");
    }
    for pkg in &packages {
        out.push_str(&format!("Installing collected packages: {}\n", pkg));
    }
    out.push_str("Successfully installed ");
    out.push_str(&packages.map(|p| format!("{}-5.0.0", p)).join(" "));
    out.push('\n');
    out
}

/// `helm install` — verbose deploy output with many noise lines.
fn helm_install_output() -> String {
    let mut out = String::new();
    out.push_str("Release \"myapp\" does not exist. Installing it now.\n");
    out.push_str("W0115 10:23:45.123456   12345 warnings.go:70] unknown field \"spec.template.spec.containers[0].resources\"\n");
    out.push_str("W0115 10:23:45.234567   12345 warnings.go:70] annotation \"kubernetes.io/change-cause\" is deprecated\n");
    out.push_str("coalesce.go:199: warning: destination for .Values.image.tag is a table. Merging, skipping value\n");
    out.push_str("NAME: myapp\n");
    out.push_str("LAST DEPLOYED: Mon Jan 15 10:23:45 2024\n");
    out.push_str("NAMESPACE: production\n");
    out.push_str("STATUS: deployed\n");
    out.push_str("REVISION: 1\n");
    out.push_str("NOTES:\n");
    out.push_str("1. Get the application URL by running these commands:\n");
    out.push_str("  export POD_NAME=$(kubectl get pods --namespace production -l app=myapp -o jsonpath=\"{.items[0].metadata.name}\")\n");
    out.push_str("  echo \"Visit http://127.0.0.1:8080 to use your application\"\n");
    out.push_str("  kubectl --namespace production port-forward $POD_NAME 8080:80\n");
    out
}

/// `gradle build` — verbose Gradle output with task headers and noise.
fn gradle_build_output() -> String {
    let mut out = String::new();
    out.push_str("Starting Gradle Daemon...\n");
    out.push_str("Gradle Daemon started in 1.234 s\n\n");
    out.push_str("> Configure project :\n");
    out.push_str("WARNING: The `compile` configuration has been deprecated for dependency declaration.\n\n");
    let tasks = [
        ":compileJava", ":processResources", ":classes",
        ":compileTestJava", ":processTestResources", ":testClasses",
        ":test", ":jar", ":assemble", ":check", ":build",
    ];
    for task in &tasks {
        out.push_str(&format!("> Task {}\n", task));
    }
    // Many UP-TO-DATE tasks from subprojects
    let subprojects = ["api", "core", "service", "web", "common"];
    for sub in &subprojects {
        for task in &tasks {
            out.push_str(&format!("> Task :{}{} UP-TO-DATE\n", sub, task));
        }
    }
    out.push_str("\nBUILD SUCCESSFUL in 45s\n");
    out.push_str("42 actionable tasks: 11 executed, 31 up-to-date\n");
    out
}

// ─── frontend build tools fixtures ───────────────────────────────────────────

fn vite_build_output() -> String {
    let mut out = String::new();
    out.push_str("vite v5.2.0 building for production...\n");
    out.push_str("transforming...\n");
    // simulate transform log lines — lots of per-file noise
    let modules = [
        "src/main.tsx", "src/App.tsx", "src/components/Button.tsx",
        "src/components/Modal.tsx", "src/components/Sidebar.tsx",
        "src/components/Header.tsx", "src/components/Footer.tsx",
        "src/components/Card.tsx", "src/components/Table.tsx",
        "src/components/Form.tsx", "src/components/Input.tsx",
        "src/components/Select.tsx", "src/components/Checkbox.tsx",
        "src/components/Radio.tsx", "src/components/Tooltip.tsx",
        "src/components/Dropdown.tsx", "src/components/Avatar.tsx",
        "src/components/Badge.tsx", "src/components/Alert.tsx",
        "src/components/Spinner.tsx", "src/pages/Home.tsx",
        "src/pages/Dashboard.tsx", "src/pages/Profile.tsx",
        "src/pages/Settings.tsx", "src/pages/Login.tsx",
        "src/pages/Register.tsx", "src/pages/NotFound.tsx",
        "src/hooks/useAuth.ts", "src/hooks/useTheme.ts",
        "src/hooks/useLocalStorage.ts", "src/hooks/useDebounce.ts",
        "src/hooks/useFetch.ts", "src/hooks/useForm.ts",
        "src/stores/auth.ts", "src/stores/ui.ts",
        "src/stores/data.ts", "src/utils/api.ts",
        "src/utils/format.ts", "src/utils/validators.ts",
        "src/utils/constants.ts",
    ];
    for m in &modules {
        out.push_str(&format!("✓ {}\n", m));
    }
    out.push_str(&format!("✓ {} modules transformed.\n", modules.len()));
    out.push_str("rendering chunks...\n");
    // chunk output
    let chunks = [
        ("dist/assets/index-DiwrgTda.js",    "142.30 kB │ gzip:  45.80 kB"),
        ("dist/assets/vendor-BKbdCLth.js",   "312.45 kB │ gzip:  98.12 kB"),
        ("dist/assets/router-C9dFYmek.js",    "24.80 kB │ gzip:   8.33 kB"),
        ("dist/assets/charts-BpHoEHuC.js",  "198.60 kB │ gzip:  60.44 kB"),
        ("dist/assets/icons-D3mNoPLJ.js",    "52.10 kB │ gzip:  14.22 kB"),
        ("dist/assets/index-CKttPMtA.css",   "28.40 kB │ gzip:   6.11 kB"),
    ];
    for (name, size) in &chunks {
        out.push_str(&format!("dist/{} │ {}\n", name, size));
    }
    out.push_str("\n✓ built in 4.32s\n");
    out
}

fn webpack_build_output() -> String {
    let mut out = String::new();
    out.push_str("asset main.js 1.44 MiB [emitted] (name: main)\n");
    out.push_str("asset vendors.react.js 312 KiB [emitted] (name: vendors-react)\n");
    out.push_str("asset vendors.utils.js 98 KiB [emitted] (name: vendors-utils)\n");
    out.push_str("asset styles.css 32.4 KiB [emitted] (name: styles)\n");
    out.push_str("asset index.html 1.23 KiB [emitted]\n");
    // Module resolution noise — what webpack spits out verbosely
    out.push_str("cacheable modules 3.21 MiB\n");
    out.push_str("  modules by path ./node_modules/ 2.98 MiB\n");
    let node_modules = [
        "react/index.js", "react-dom/index.js", "react-router-dom/index.js",
        "axios/index.js", "lodash/lodash.js", "moment/moment.js",
        "date-fns/index.js", "classnames/index.js", "immer/dist/immer.cjs.js",
        "zustand/dist/zustand.cjs.js", "react-query/lib/index.js",
        "react-hook-form/dist/index.cjs.js", "zod/lib/index.js",
        "react-table/dist/react-table.development.js",
        "@mui/material/index.js", "@mui/icons-material/index.js",
        "framer-motion/dist/framer-motion.cjs.js",
        "react-select/dist/react-select.cjs.js",
        "recharts/lib/index.js", "react-virtualized/dist/commonjs/index.js",
    ];
    for m in &node_modules {
        out.push_str(&format!("    ./node_modules/{} 65.7 KiB [built] [code generated]\n", m));
    }
    out.push_str("  modules by path ./src/ 234 KiB\n");
    let src_modules = [
        "src/index.tsx", "src/App.tsx", "src/router.tsx",
        "src/store/index.ts", "src/api/client.ts", "src/api/auth.ts",
        "src/api/users.ts", "src/api/products.ts",
        "src/components/Button/index.tsx", "src/components/Modal/index.tsx",
        "src/pages/Dashboard/index.tsx", "src/pages/Profile/index.tsx",
    ];
    for m in &src_modules {
        out.push_str(&format!("    ./{} 12.3 KiB [built] [code generated]\n", m));
    }
    out.push_str("WARNING in ./src/utils/legacy.js\n");
    out.push_str("  DeprecationWarning: Buffer() is deprecated\n\n");
    out.push_str("WARNING in ./node_modules/some-lib/index.js\n");
    out.push_str("  Critical dependency: the request of a CommonJS module\n\n");
    out.push_str("webpack 5.91.0 compiled with 2 warnings in 18432 ms\n");
    out
}

fn turbo_run_output() -> String {
    let mut out = String::new();
    out.push_str("• Packages in scope: web, docs, @repo/ui, @repo/utils, @repo/config\n");
    out.push_str("• Running build in 5 packages\n");
    out.push_str("• Remote caching enabled\n\n");

    let tasks = [
        ("@repo/config:build",  "cache hit,  replaying output 1a2b3c4d"),
        ("@repo/utils:build",   "cache hit,  replaying output 5e6f7a8b"),
        ("@repo/ui:build",      "cache miss, executing 9c0d1e2f"),
        ("docs:build",          "cache miss, executing 3a4b5c6d"),
        ("web:build",           "cache miss, executing 7e8f9a0b"),
    ];

    for (task, status) in &tasks {
        out.push_str(&format!("{}: {}\n", task, status));
        // Inner build output that should be stripped
        out.push_str(&format!("{}: > {} build\n", task, task.split(':').next().unwrap_or("")));
        if task.contains("ui") {
            out.push_str(&format!("{}: > tsc --noEmit\n", task));
            out.push_str(&format!("{}: ✓ TypeScript compilation complete\n", task));
            out.push_str(&format!("{}: > rollup -c\n", task));
            out.push_str(&format!("{}: created dist/index.js in 2.3s\n", task));
            out.push_str(&format!("{}: created dist/index.esm.js in 2.4s\n", task));
        } else if task.contains("docs") {
            out.push_str(&format!("{}: > next build\n", task));
            out.push_str(&format!("{}: ✓ Generating static pages (24/24)\n", task));
            out.push_str(&format!("{}: Route (pages)  Size  First Load JS\n", task));
        } else if task.contains("web") {
            out.push_str(&format!("{}: > next build\n", task));
            for p in &["/", "/about", "/blog", "/contact", "/pricing"] {
                out.push_str(&format!("{}: ○ {}\n", task, p));
            }
        }
        out.push_str(&format!("{}:\n", task));
    }

    out.push_str("\n Tasks:    5 successful, 5 total\n");
    out.push_str("  Cached:    2 cached, 5 total\n");
    out.push_str("    Time:    42.117s >>> FULL TURBO\n");
    out
}

fn stylelint_output() -> String {
    let mut out = String::new();
    let files = [
        "src/components/Button/Button.css",
        "src/components/Modal/Modal.scss",
        "src/components/Sidebar/Sidebar.css",
        "src/pages/Dashboard/Dashboard.scss",
        "src/pages/Profile/Profile.css",
        "src/styles/global.scss",
        "src/styles/variables.scss",
        "src/styles/typography.css",
    ];
    let rules = [
        ("✖", "error",   "Unexpected unknown property \"colour\"",          "property-no-unknown"),
        ("⚠", "warning", "Expected a leading zero",                         "number-leading-zero"),
        ("✖", "error",   "Unexpected empty block",                          "block-no-empty"),
        ("⚠", "warning", "Expected single-line comment to be \"//\"",       "comment-no-double-slash"),
        ("✖", "error",   "Unexpected invalid hex color \"#gggggg\"",        "color-no-invalid-hex"),
        ("⚠", "warning", "Expected no more than 2 empty lines",             "max-empty-lines"),
        ("✖", "error",   "Unexpected longhand property \"border-top-width\"","shorthand-property-no-redundant-values"),
    ];
    let mut total = 0usize;
    for (fi, file) in files.iter().enumerate() {
        out.push_str(&format!("{}\n", file));
        for j in 0..5usize {
            let (sym, level, msg, rule) = rules[(fi * 5 + j) % rules.len()];
            let line = (j + 1) * 3;
            out.push_str(&format!("  {}:{:2}  {}  {}  {}\n", line, j + 1, sym, level, msg));
            out.push_str(&format!("         {}  {}\n", " ".repeat(msg.len()), rule));
            total += 1;
        }
        out.push_str("\n");
    }
    out.push_str(&format!("{} problems ({} errors, {} warnings)\n",
        total, total * 3 / 5, total * 2 / 5));
    out
}

fn biome_output() -> String {
    let files = [
        ("src/App.tsx",                   15,  5,  "lint/correctness/noUnusedVariables",  "This variable is unused."),
        ("src/components/Button.tsx",     23,  3,  "lint/a11y/useButtonType",              "Provide an explicit type prop for the button element."),
        ("src/components/Modal.tsx",      41,  7,  "lint/correctness/useExhaustiveDeps",  "This hook has missing dependencies."),
        ("src/pages/Dashboard.tsx",       88, 12,  "lint/style/noNegationElse",            "Invert the condition to avoid negation."),
        ("src/pages/Login.tsx",           34,  4,  "lint/suspicious/noDoubleEquals",       "Use === instead of ==."),
        ("src/hooks/useAuth.ts",          56,  9,  "lint/correctness/noUnusedVariables",  "This variable is unused."),
        ("src/utils/api.ts",              12,  1,  "lint/security/noGlobalEval",           "eval() is a security risk."),
        ("src/store/authSlice.ts",        67, 14,  "lint/style/useConst",                  "This variable is never reassigned. Use const instead."),
        ("src/components/Table.tsx",      29,  2,  "lint/a11y/useKeyWithClickEvents",      "Pair this with an onKeyDown handler."),
        ("src/components/Form.tsx",      102,  8,  "lint/correctness/noUnusedVariables",  "This variable is unused."),
    ];

    let mut out = String::new();
    for (file, line, col, rule, msg) in &files {
        // Header line with separator
        out.push_str(&format!("./{file}:{line}:{col} {rule} ━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━\n\n"));
        out.push_str(&format!("  ✖ {}\n\n", msg));
        // Code context (3 lines: before, offending, underline)
        out.push_str(&format!("  {:>3} │ const placeholder_{} = \"value\";\n", line - 1, line));
        out.push_str(&format!("  {:>3} │ const example_{} = \"code here\";\n", line, line));
        out.push_str(&format!("      │   ^^^^^^^^^^^^^^^^^^^^^^^^^^^^^\n"));
        out.push_str(&format!("  {:>3} │ // next line\n\n", line + 1));
        out.push_str("  ℹ Unsafe fix: apply suggested change.\n\n");
        out.push_str(&format!("  {:>3} │ const example_{} = \"code here\";\n", line, line));
        out.push_str(&format!("      │ - const example_{} = \"code here\";\n", line));
        out.push_str(&format!("      │ + const _example_{} = \"code here\";\n\n", line));
    }
    out.push_str(&format!("Checked {} files in 342ms.\n", files.len() + 5));
    out.push_str(&format!("Found {} diagnostics.\n", files.len()));
    out
}

// ─── benchmark runner ────────────────────────────────────────────────────────

#[test]
fn benchmark_handlers() {
    let git        = GitHandler;
    let cargo      = CargoHandler;
    let tsc        = TscHandler;
    let ls         = LsHandler;
    let jest       = JestHandler;
    let pytest     = PytestHandler;
    let vitest     = VitestHandler;
    let eslint     = EslintHandler;
    let npm        = NpmHandler;
    let kubectl    = KubectlHandler;
    let terraform  = TerraformHandler;
    let docker     = DockerHandler;
    let make       = MakeHandler;
    let gh         = GhHandler;
    let grep       = GrepHandler;
    let brew       = BrewHandler;
    let go         = GoHandler;
    let maven      = MavenHandler;
    let gradle     = GradleHandler;
    let helm       = HelmHandler;
    let env        = EnvHandler;
    let clippy     = ClippyHandler;
    let golangci   = GolangCiLintHandler;
    let next       = NextHandler;
    let playwright = PlaywrightHandler;
    let pip        = PipHandler;
    let ember      = EmberHandler;
    let vite       = ViteHandler;
    let webpack    = WebpackHandler;
    let turbo      = TurboHandler;
    let stylelint  = StylelintHandler;
    let biome      = BiomeHandler;

    let (cargo_baseline, cargo_json) = cargo_build();
    let cargo_test_raw               = cargo_test();
    let (status_baseline, porcelain) = git_status();
    let (log_baseline, log_oneline)  = git_log();
    let diff_raw                     = git_diff_large();
    let push_raw                     = git_push();
    let ls_raw                       = ls_project();
    let tsc_raw                      = tsc_errors();
    let jest_raw                     = jest_output();

    let pytest_raw     = pytest_output();
    let vitest_raw     = vitest_output();
    let eslint_raw     = eslint_output();
    let npm_raw        = npm_install_output();
    let kubectl_raw    = kubectl_pods();
    let terraform_raw  = terraform_plan();
    let docker_raw     = docker_ps_output();
    let make_raw       = make_build_output();
    let gh_raw         = gh_pr_list_output();
    let grep_raw       = grep_many_matches();
    let brew_raw       = brew_install_output();
    let ember_raw      = ember_build_output();
    let go_raw         = go_test_output();
    let maven_raw      = maven_output();
    let clippy_raw     = clippy_verbose();
    let golangci_raw   = golangci_output();
    let next_raw       = next_build_output();
    let playwright_raw = playwright_output();
    let pip_raw        = pip_install_output();
    let env_raw        = env_large();
    let helm_raw       = helm_install_output();
    let gradle_raw     = gradle_build_output();
    let vite_raw       = vite_build_output();
    let webpack_raw    = webpack_build_output();
    let turbo_raw      = turbo_run_output();
    let stylelint_raw  = stylelint_output();
    let biome_raw      = biome_output();

    struct Row { op: &'static str, in_tok: usize, out_tok: usize, min_pct: f64 }

    macro_rules! row {
        ($op:expr, $handler:expr, $input:expr, $args:expr, $min:expr) => {{
            let out = run(&$handler, &$input, $args);
            Row { op: $op, in_tok: count_tokens(&$input), out_tok: count_tokens(&out), min_pct: $min }
        }};
        // variant for baseline != handler_input
        (baseline=$base:expr; $op:expr, $handler:expr, $input:expr, $args:expr, $min:expr) => {{
            let out = run(&$handler, &$input, $args);
            Row { op: $op, in_tok: count_tokens(&$base), out_tok: count_tokens(&out), min_pct: $min }
        }};
    }

    let rows: Vec<Row> = vec![
        // ── Rust / Cargo ────────────────────────────────────────────────────
        row!(baseline=cargo_baseline; "cargo build", cargo, cargo_json, &["cargo","build"], 80.0),
        row!("cargo test", cargo, cargo_test_raw, &["cargo","test"], 80.0),
        row!("cargo clippy", clippy, clippy_raw, &["clippy"], 30.0),
        // ── Git ──────────────────────────────────────────────────────────────
        row!(baseline=status_baseline; "git status", git, porcelain, &["git","status"], 30.0),
        row!(baseline=log_baseline; "git log", git, log_oneline, &["git","log"], 50.0),
        row!("git diff", git, diff_raw, &["git","diff"], 40.0),
        row!("git push", git, push_raw, &["git","push"], 10.0),
        // ── JavaScript / TypeScript ──────────────────────────────────────────
        row!("tsc", tsc, tsc_raw, &["tsc"], 40.0),
        row!("jest", jest, jest_raw, &["jest"], 50.0),
        row!("vitest", vitest, vitest_raw, &["vitest"], 50.0),
        row!("eslint", eslint, eslint_raw, &["eslint"], 60.0),
        row!("npm install", npm, npm_raw, &["npm","install"], 30.0),
        // ── Next.js ──────────────────────────────────────────────────────────
        row!("next build", next, next_raw, &["next","build"], 30.0),
        // ── Python ───────────────────────────────────────────────────────────
        row!("pytest", pytest, pytest_raw, &["pytest"], 80.0),
        row!("pip install", pip, pip_raw, &["pip","install"], 30.0),
        // ── Ember ─────────────────────────────────────────────────────────────
        row!("ember build", ember, ember_raw, &["ember","build"], 70.0),
        // ── Go ───────────────────────────────────────────────────────────────
        row!("go test", go, go_raw, &["go","test"], 50.0),
        row!("golangci-lint", golangci, golangci_raw, &["golangci-lint"], 60.0),
        // ── Java / JVM ───────────────────────────────────────────────────────
        row!("mvn install", maven, maven_raw, &["mvn","install"], 55.0),
        row!("gradle build", gradle, gradle_raw, &["gradle","build"], 40.0),
        // ── DevOps ───────────────────────────────────────────────────────────
        row!("kubectl get pods", kubectl, kubectl_raw, &["kubectl","get"], 10.0),
        row!("terraform plan", terraform, terraform_raw, &["terraform","plan"], 60.0),
        row!("docker ps", docker, docker_raw, &["docker","ps"], 10.0),
        row!("make", make, make_raw, &["make"], 30.0),
        row!("helm install", helm, helm_raw, &["helm","install"], 10.0),
        // ── GitHub CLI ───────────────────────────────────────────────────────
        row!("gh pr list", gh, gh_raw, &["gh","pr","list"], 10.0),
        // ── System / Utilities ───────────────────────────────────────────────
        row!("ls", ls, ls_raw, &["ls"], 70.0),
        row!("grep", grep, grep_raw, &["grep"], 10.0),
        row!("brew install", brew, brew_raw, &["brew","install"], 10.0),
        // ── Testing / QA ─────────────────────────────────────────────────────
        row!("playwright test", playwright, playwright_raw, &["playwright","test"], 30.0),
        // ── Environment ──────────────────────────────────────────────────────
        row!("env", env, env_raw, &["env"], 45.0),
        // ── Frontend build tools ─────────────────────────────────────────────
        row!("vite build", vite, vite_raw, &["vite","build"], 50.0),
        row!("webpack", webpack, webpack_raw, &["webpack"], 70.0),
        row!("turbo run build", turbo, turbo_raw, &["turbo","run","build"], 50.0),
        row!("stylelint", stylelint, stylelint_raw, &["stylelint"], 20.0),
        row!("biome lint", biome, biome_raw, &["biome","lint"], 45.0),
    ];

    println!();
    println!("{:<30} {:>12} {:>10} {:>10}", "Operation", "Without Panda", "With Panda", "Savings");
    println!("{}", "─".repeat(66));

    let mut total_in  = 0usize;
    let mut total_out = 0usize;

    for row in &rows {
        let pct = savings_pct(row.in_tok, row.out_tok);
        println!("{:<30} {:>12} {:>10} {:>9.0}%",
            row.op, row.in_tok, row.out_tok, pct);
        total_in  += row.in_tok;
        total_out += row.out_tok;
    }

    println!("{}", "─".repeat(66));
    let total_pct = savings_pct(total_in, total_out);
    println!("{:<30} {:>12} {:>10} {:>9.0}%", "TOTAL", total_in, total_out, total_pct);
    println!();

    // Per-handler assertions using each row's declared minimum
    for row in &rows {
        let pct = savings_pct(row.in_tok, row.out_tok);
        assert!(
            pct >= row.min_pct,
            "Handler for '{}' saved only {:.0}% — expected ≥{:.0}%",
            row.op, pct, row.min_pct
        );
    }
}

// ─── Fix 1: pipeline medium-output threshold benchmark ───────────────────────

/// 80-line cargo test fixture: 74 passing tests + a realistic failure block.
/// Sized to fall in the old dead zone (51–199 lines).
fn medium_cargo_test_failure() -> String {
    let mut out = String::new();
    for i in 0..74usize {
        out.push_str(&format!("test api::tests::test_case_{:02} ... ok\n", i));
    }
    out.push_str("test auth::tests::test_jwt_expiry ... FAILED\n");
    out.push_str("\nfailures:\n\n");
    out.push_str("---- auth::tests::test_jwt_expiry stdout ----\n");
    out.push_str("thread 'auth::tests::test_jwt_expiry' panicked at \
                  'assertion failed: token.is_valid()'\n");
    out.push_str("src/auth/jwt.rs:156:9\n");
    out.push_str("note: run with `RUST_BACKTRACE=1` for a backtrace\n\n");
    out.push_str("failures:\n");
    out.push_str("    auth::tests::test_jwt_expiry\n\n");
    out.push_str("test result: FAILED. 74 passed; 1 failed; 0 ignored; finished in 3.12s\n");
    out
}

#[test]
fn benchmark_pipeline_medium_output_threshold() {
    use panda_core::config::{CcrConfig, GlobalConfig};
    use panda_core::pipeline::Pipeline;

    let fixture = medium_cargo_test_failure();
    let line_count = fixture.lines().count();
    let in_tok = count_tokens(&fixture);

    // "before" — old 200-line threshold, BERT skipped for this fixture
    let mut global_old = GlobalConfig::default();
    global_old.summarize_threshold_lines = 200;
    let config_old = CcrConfig { global: global_old, ..CcrConfig::default() };
    let result_old = Pipeline::new(config_old)
        .process(&fixture, Some("cargo"), None, None)
        .unwrap();
    let out_tok_old = count_tokens(&result_old.output);

    // "after" — new 50-line threshold, BERT active
    let result_new = Pipeline::new(CcrConfig::default())
        .process(&fixture, Some("cargo"), None, None)
        .unwrap();
    let out_tok_new = count_tokens(&result_new.output);

    println!();
    println!("── Fix 1: Medium output ({} lines, cargo test failure) ──", line_count);
    println!("{:<30} {:>12} {:>10} {:>10}", "Threshold", "Without Panda", "With Panda", "Savings");
    println!("{}", "─".repeat(66));
    println!("{:<30} {:>12} {:>10} {:>9.0}%", "Old (200-line threshold)", in_tok, out_tok_old, savings_pct(in_tok, out_tok_old));
    println!("{:<30} {:>12} {:>10} {:>9.0}%", "New  (50-line threshold)", in_tok, out_tok_new, savings_pct(in_tok, out_tok_new));
    println!();

    // Critical lines must survive BERT
    assert!(
        result_new.output.contains("FAILED") || result_new.output.contains("panicked"),
        "failure details must survive BERT summarization"
    );
    // New threshold must save more than old (old saved ~0% on this fixture)
    assert!(
        out_tok_new < out_tok_old,
        "new 50-line threshold should compress more than old 200-line threshold \
         (new={} old={})", out_tok_new, out_tok_old
    );
    // Should save at least 40% on this fixture
    assert!(
        savings_pct(in_tok, out_tok_new) >= 40.0,
        "expected ≥40% savings on 80-line cargo failure, got {:.0}%",
        savings_pct(in_tok, out_tok_new)
    );
}

#[test]
fn benchmark_pipeline_short_output_passthrough() {
    use panda_core::pipeline::Pipeline;
    use panda_core::config::CcrConfig;

    // 30 lines — well below the new 50-line threshold, must not be compressed
    let fixture: String = (0..30)
        .map(|i| format!("test module::test_case_{:02} ... ok", i))
        .collect::<Vec<_>>()
        .join("\n");

    let result = Pipeline::new(CcrConfig::default())
        .process(&fixture, Some("cargo"), None, None)
        .unwrap();

    assert!(
        result.output.lines().count() >= 28,
        "outputs below 50-line threshold must not be BERT-compressed, \
         got {} lines from {} lines input",
        result.output.lines().count(),
        fixture.lines().count()
    );
}

// ─── Fix 2: chunked budget consolidation benchmark ───────────────────────────

/// Large gradle/maven-style build log with 25 submodules × ~100 lines each = ~2500 lines.
/// Exceeds CHUNK_THRESHOLD_LINES (2000) so chunked processing triggers.
fn large_gradle_build() -> String {
    let mut out = String::new();
    out.push_str("Starting Gradle Daemon...\n");
    out.push_str("Gradle Daemon started in 2.341 s\n\n");
    let tasks = [
        "compileJava", "processResources", "classes",
        "compileTestJava", "processTestResources", "testClasses",
        "test", "jar", "assemble", "check",
    ];
    let submodules: Vec<String> = (0..25).map(|i| format!("module-{:02}", i)).collect();
    for sub in &submodules {
        out.push_str(&format!("> Configure project :{}\n", sub));
        out.push_str(&format!("> Task :{}\n", sub));
        for task in &tasks {
            out.push_str(&format!("> Task :{}:{}\n", sub, task));
            // Simulate test output per task
            for j in 0..6usize {
                out.push_str(&format!(
                    "{}:{}:{} > TestClass > test_method_{:02}() PASSED\n",
                    sub, sub, task, j
                ));
            }
            out.push_str(&format!(
                "{}:{} > {} tests completed, 0 failed\n", sub, task, 6
            ));
        }
        out.push_str(&format!(
            "BUILD SUCCESSFUL for :{} in 12s\n", sub
        ));
    }
    out.push_str("\nBUILD SUCCESSFUL in 187s\n");
    out.push_str(&format!("{} actionable tasks: {} executed, {} up-to-date\n",
        submodules.len() * tasks.len(),
        submodules.len() * 3,
        submodules.len() * (tasks.len() - 3),
    ));
    out
}

#[test]
fn benchmark_pipeline_chunked_budget() {
    use panda_core::config::CcrConfig;
    use panda_core::pipeline::Pipeline;

    let fixture = large_gradle_build();
    let line_count = fixture.lines().count();
    let in_tok = count_tokens(&fixture);
    let intended_budget = 60usize; // head_lines(30) + tail_lines(30)

    let result = Pipeline::new(CcrConfig::default())
        .process(&fixture, Some("gradle"), None, None)
        .unwrap();
    let out_lines = result.output.lines().count();
    let out_tok = count_tokens(&result.output);

    println!();
    println!("── Fix 2: Chunked build log ({} lines) ──", line_count);
    println!("{:<30} {:>12} {:>10} {:>10}", "Pass", "Without Panda", "With Panda", "Savings");
    println!("{}", "─".repeat(66));
    println!("{:<30} {:>12} {:>10} {:>9.0}%",
        "Chunked+consolidated", in_tok, out_tok, savings_pct(in_tok, out_tok));
    println!("Output lines: {} (budget was {})", out_lines, intended_budget);
    println!();

    // Without consolidation, N chunks × 60 lines = could be 300+ lines.
    // With consolidation, output must be within 2× the intended budget.
    assert!(
        out_lines <= intended_budget * 2,
        "consolidated output ({} lines) exceeded 2× budget ({} lines)",
        out_lines, intended_budget * 2
    );
    // Should save substantial tokens on a repetitive build log
    assert!(
        savings_pct(in_tok, out_tok) >= 75.0,
        "expected ≥75% savings on large build log, got {:.0}%",
        savings_pct(in_tok, out_tok)
    );
}

// ─── Fix 3: Grep tool handler benchmark ──────────────────────────────────────

#[test]
fn benchmark_grep_tool_handler() {
    // Uses the existing grep_many_matches() fixture: 150 lines in file:line:content format,
    // which is exactly what the Claude Code Grep tool emits in content mode.
    let input = grep_many_matches();
    let in_tok = count_tokens(&input);

    // Simulate what process_grep() does: run GrepHandler directly
    let handler = GrepHandler;
    let out = handler.filter(&input, &["grep".to_string(), "handle_request".to_string()]);
    let out_tok = count_tokens(&out);

    println!();
    println!("── Fix 3: Grep tool (150 matches, 10 files) ──");
    println!("{:<30} {:>12} {:>10} {:>10}", "Pass", "Without Panda", "With Panda", "Savings");
    println!("{}", "─".repeat(66));
    println!("{:<30} {:>12} {:>10} {:>9.0}%",
        "GrepHandler (file:line:content)", in_tok, out_tok, savings_pct(in_tok, out_tok));
    println!();

    // Must group by file and apply caps
    assert!(out.contains("src/api/users.rs"), "file grouping must be present");
    // Must save tokens — short filenames (16-18 chars) don't trigger compact_path,
    // so savings come only from removing the filename prefix on each match line (~14%).
    assert!(
        savings_pct(in_tok, out_tok) >= 12.0,
        "expected ≥12% savings on 150-match grep output, got {:.0}%",
        savings_pct(in_tok, out_tok)
    );
}

#[test]
fn benchmark_grep_tool_short_passthrough() {
    // Grep tool with ≤10 results passes through unchanged (no handler overhead for trivial results)
    let input = "src/main.rs:42:fn main() {\nsrc/lib.rs:10:fn helper() {";
    let handler = GrepHandler;
    let out = handler.filter(input, &["grep".to_string()]);
    // 2 lines: the GrepHandler should still work fine (just groups them)
    assert!(!out.is_empty());
}

// ─── New handler fixtures ─────────────────────────────────────────────────────

/// `uv sync` on a large Django + ML project (100 packages, 85 downloads).
/// Mirrors real `uv sync` output: Python version banner, venv creation, many
/// Downloading/Fetching/Preparing lines, Resolved/Audited noise, then the
/// final "Installed N packages" summary.
fn uv_install_large() -> String {
    let packages = [
        ("django",              "4.2.7",   "7.2 MB"),
        ("djangorestframework", "3.14.0",  "1.1 MB"),
        ("celery",              "5.3.4",   "1.4 MB"),
        ("redis",               "5.0.1",   "252 kB"),
        ("psycopg2-binary",     "2.9.9",   "3.1 MB"),
        ("pillow",              "10.1.0",  "46.9 MB"),
        ("boto3",               "1.34.0",  "1.3 MB"),
        ("botocore",            "1.34.0",  "10.4 MB"),
        ("s3transfer",          "0.8.2",   "84 kB"),
        ("jmespath",            "1.0.1",   "24 kB"),
        ("numpy",               "1.26.2",  "17.1 MB"),
        ("pandas",              "2.1.3",   "11.8 MB"),
        ("scikit-learn",        "1.3.2",   "10.3 MB"),
        ("scipy",               "1.11.4",  "36.3 MB"),
        ("matplotlib",          "3.8.2",   "7.4 MB"),
        ("seaborn",             "0.13.0",  "292 kB"),
        ("requests",            "2.31.0",  "62 kB"),
        ("httpx",               "0.25.2",  "75 kB"),
        ("pydantic",            "2.5.2",   "398 kB"),
        ("pydantic-core",       "2.14.5",  "1.8 MB"),
        ("fastapi",             "0.104.1", "91 kB"),
        ("uvicorn",             "0.24.0",  "60 kB"),
        ("starlette",           "0.27.0",  "71 kB"),
        ("anyio",               "4.1.0",   "86 kB"),
        ("sniffio",             "1.3.0",   "15 kB"),
        ("sqlalchemy",          "2.0.23",  "9.4 MB"),
        ("alembic",             "1.12.1",  "1.1 MB"),
        ("mako",                "1.3.0",   "381 kB"),
        ("greenlet",            "3.0.1",   "297 kB"),
        ("cryptography",        "41.0.7",  "4.1 MB"),
        ("cffi",                "1.16.0",  "476 kB"),
        ("pycparser",           "2.21",    "173 kB"),
        ("paramiko",            "3.3.1",   "291 kB"),
        ("bcrypt",              "4.1.1",   "535 kB"),
        ("PyNaCl",              "1.5.0",   "1.1 MB"),
        ("python-dotenv",       "1.0.0",   "21 kB"),
        ("click",               "8.1.7",   "96 kB"),
        ("rich",                "13.7.0",  "243 kB"),
        ("typer",               "0.9.0",   "45 kB"),
        ("pytest",              "7.4.3",   "223 kB"),
        ("pytest-django",       "4.7.0",   "58 kB"),
        ("pytest-asyncio",      "0.21.1",  "49 kB"),
        ("factory-boy",         "3.3.0",   "68 kB"),
        ("faker",               "20.1.0",  "1.8 MB"),
        ("coverage",            "7.3.2",   "442 kB"),
        ("mypy",                "1.7.1",   "13.5 MB"),
        ("mypy-extensions",     "1.0.0",   "8.7 kB"),
        ("types-requests",      "2.31.0",  "32 kB"),
        ("ruff",                "0.1.7",   "7.4 MB"),
        ("black",               "23.11.0", "642 kB"),
        ("isort",               "5.13.0",  "82 kB"),
        ("flake8",              "6.1.0",   "112 kB"),
        ("pycodestyle",         "2.11.1",  "42 kB"),
        ("pyflakes",            "3.1.0",   "73 kB"),
        ("mccabe",              "0.7.0",   "10 kB"),
        ("pre-commit",          "3.6.0",   "196 kB"),
        ("identify",            "2.5.32",  "97 kB"),
        ("nodeenv",             "1.8.0",   "25 kB"),
        ("cfgv",                "3.4.0",   "7.5 kB"),
        ("virtualenv",          "20.25.0", "3.5 MB"),
        ("distlib",             "0.3.8",   "468 kB"),
        ("filelock",            "3.13.1",  "13 kB"),
        ("platformdirs",        "4.1.0",   "19 kB"),
        ("setuptools",          "69.0.2",  "2.3 MB"),
        ("wheel",               "0.41.3",  "37 kB"),
        ("pip",                 "23.3.2",  "2.3 MB"),
        ("packaging",           "23.2",    "51 kB"),
        ("certifi",             "2023.11.17", "162 kB"),
        ("charset-normalizer",  "3.3.2",   "138 kB"),
        ("idna",                "3.6",     "62 kB"),
        ("urllib3",             "2.1.0",   "101 kB"),
        ("six",                 "1.16.0",  "12 kB"),
        ("python-dateutil",     "2.8.2",   "236 kB"),
        ("pytz",                "2023.3.post1", "2.2 MB"),
        ("tzdata",              "2023.3",  "346 kB"),
        ("annotated-types",     "0.6.0",   "17 kB"),
        ("typing-extensions",   "4.9.0",   "74 kB"),
        ("attrs",               "23.1.0",  "105 kB"),
        ("iniconfig",           "2.0.0",   "11 kB"),
        ("pluggy",              "1.3.0",   "20 kB"),
        ("exceptiongroup",      "1.2.0",   "26 kB"),
        ("tomli",               "2.0.1",   "34 kB"),
        ("pathspec",            "0.11.2",  "39 kB"),
        ("h11",                 "0.14.0",  "50 kB"),
        ("httpcore",            "1.0.2",   "85 kB"),
        ("socksio",             "1.0.0",   "11 kB"),
    ];

    let mut out = String::new();
    out.push_str("Using CPython 3.11.6 interpreter at /usr/bin/python3.11\n");
    out.push_str("Creating virtual environment at: .venv\n");
    out.push_str(&format!("Resolved {} packages in 1.23s\n", packages.len()));
    for (name, ver, size) in &packages {
        // Alternate between Downloading, Fetching, Preparing
        let verb = match name.len() % 3 {
            0 => "Downloading",
            1 => "Fetching",
            _ => "Preparing",
        };
        out.push_str(&format!("   {} {}-{}.tar.gz ({})\n", verb, name, ver, size));
    }
    out.push_str(&format!("Installed {} packages in 14.3s\n", packages.len()));
    out.push_str(&format!("Audited {} packages in 0.02s\n", packages.len()));
    out
}

/// `ruff check` on a large Python codebase: 5 files, 80 violations.
/// Mix of E501 (many per file), F401 (unused imports), E302, W291, B006.
fn ruff_check_large() -> String {
    let files = [
        "src/api/views.py",
        "src/api/serializers.py",
        "src/models/user.py",
        "src/models/order.py",
        "src/utils/helpers.py",
        "src/tasks/celery_tasks.py",
        "src/middleware/auth.py",
        "src/core/settings.py",
        "tests/test_api.py",
        "tests/test_models.py",
    ];
    let violations = [
        ("E501", "Line too long ({} > 88 characters)"),
        ("E501", "Line too long ({} > 88 characters)"),
        ("E501", "Line too long ({} > 88 characters)"),
        ("F401", "`os` imported but unused"),
        ("F401", "`sys` imported but unused"),
        ("W291", "Trailing whitespace"),
        ("W291", "Trailing whitespace"),
        ("E302", "Expected 2 blank lines, found 1"),
        ("B006", "Do not use mutable data structures for argument defaults"),
        ("E711", "Comparison to `None` (use `is` or `is not`)"),
    ];
    let mut out = String::new();
    let mut line_no = 1usize;
    for file in &files {
        for (i, (code, msg_template)) in violations.iter().enumerate() {
            let col = 1 + (i * 4 % 40);
            let msg = if msg_template.contains("{}") {
                msg_template.replace("{}", &format!("{}", 89 + i * 3))
            } else {
                msg_template.to_string()
            };
            out.push_str(&format!("{}:{}:{}: {} {}\n", file, line_no, col, code, msg));
            line_no += 3;
        }
        line_no += 10;
    }
    out.push_str(&format!("Found {} errors.\n", files.len() * violations.len()));
    out
}

/// `mypy` on a large Python service: 8 files, 60 type errors, daemon startup,
/// notes interleaved, and summary.
fn mypy_large() -> String {
    let files = [
        ("src/api/views.py",        12usize),
        ("src/api/serializers.py",   8),
        ("src/models/user.py",       6),
        ("src/models/order.py",     15),   // exceeds MAX_ERRORS_PER_FILE cap
        ("src/tasks/celery_tasks.py", 5),
        ("src/utils/helpers.py",     4),
        ("src/core/settings.py",     3),
        ("tests/test_api.py",        7),
    ];
    let error_templates = [
        (1,  "Argument 1 to \"filter\" has incompatible type \"Optional[str]\"; expected \"str\""),
        (2,  "Item \"None\" of \"Optional[User]\" has no attribute \"id\""),
        (3,  "Incompatible return value type (got \"None\", expected \"Response\")"),
        (4,  "Cannot assign to a method  [method-assign]"),
        (5,  "Missing return statement"),
        (6,  "\"dict[str, Any]\" has no attribute \"get_or_create\""),
        (7,  "Argument 1 to \"serialize\" has incompatible type \"int\"; expected \"str\""),
        (8,  "Incompatible types in assignment (expression has type \"str\", variable has type \"int\")"),
        (9,  "Module has no attribute \"celery_app\""),
        (10, "Need type annotation for \"results\" (hint: \"results: List[<type>] = ...\")"),
        (11, "Unsupported operand types for + (\"str\" and \"int\")"),
        (12, "Value of type \"Optional[QuerySet]\" is not indexable"),
        (13, "\"Callable[..., Any]\" not callable"),
        (14, "Name \"config\" is not defined"),
        (15, "Too many arguments for \"__init__\" of \"Model\""),
    ];

    let mut out = String::new();
    out.push_str("Daemon started successfully\n");
    let mut total_errors = 0usize;
    for (file, count) in &files {
        for i in 0..*count {
            let line_no = 10 + i * 7;
            let (_, msg) = error_templates[i % error_templates.len()];
            out.push_str(&format!("{}:{}: error: {}  [misc]\n", file, line_no, msg));
            // Interleave notes
            if i % 3 == 0 {
                out.push_str(&format!("{}:{}: note: See https://mypy.rtfd.io\n", file, line_no + 1));
            }
            total_errors += 1;
        }
    }
    out.push_str(&format!(
        "Found {} errors in {} files (checked 24 source files)\n",
        total_errors,
        files.len()
    ));
    out
}

/// `nx run-many --target=build,test,lint` across 20 packages.
/// 17 passing (10 cached), 1 failing with real error output, 2 slow non-cached.
fn nx_run_many_large() -> String {
    let projects = [
        "apps/web",      "apps/api",      "apps/admin",    "apps/mobile",
        "libs/ui",       "libs/utils",    "libs/auth",     "libs/api-client",
        "libs/db",       "libs/config",   "libs/testing",  "libs/i18n",
        "libs/hooks",    "libs/types",    "libs/schemas",  "libs/validators",
        "libs/services", "libs/analytics","libs/cache",    "libs/queue",
    ];
    let mut out = String::new();
    out.push_str("   NX   Nx Cloud enabled\n");
    out.push_str("\n");
    out.push_str("   NX   Running target build for 20 projects and 1 task they depend on...\n");
    out.push_str("\n");

    for (i, project) in projects.iter().enumerate() {
        out.push_str(&"─".repeat(60));
        out.push_str("\n");
        // First project is the failing one
        if i == 0 {
            out.push_str(&format!("> nx run {}:build\n", project));
            out.push_str("\n");
            out.push_str("  vite v5.0.8 building for production...\n");
            out.push_str("  transforming...\n");
            out.push_str("  ERROR: Build failed with errors.\n");
            out.push_str("  error during build:\n");
            out.push_str("  RollupError: Could not resolve \"@libs/missing\" from \"src/App.tsx\"\n");
            out.push_str("    at getRollupEror (node_modules/rollup/dist/shared/rollup.js:198:19)\n");
            out.push_str("    at error (node_modules/rollup/dist/shared/rollup.js:195:24)\n");
            out.push_str("    at ModuleLoader.resolveId (node_modules/rollup/dist/shared/rollup.js:22610:13)\n");
        } else {
            // Cached for first 10 passing, non-cached for remaining
            let cached = i <= 10;
            out.push_str(&format!("> nx run {}:build{}\n", project, if cached { " [local cache]" } else { "" }));
            out.push_str("\n");
            if cached {
                out.push_str("  Nx read the output from the cache instead of running the command for 1 out of 1 tasks.\n");
            } else {
                out.push_str("  vite v5.0.8 building for production...\n");
                out.push_str("  transforming (342 modules)...\n");
                out.push_str(&format!("  ✓ {} modules transformed.\n", 200 + i * 12));
                out.push_str(&format!("  dist/{}/assets/index-Dh3GX4nq.js    {:>6} kB │ gzip: {:>5} kB\n",
                    project.replace('/', "-"), 284 + i, 91 + i));
                out.push_str(&format!("  dist/{}/index.html                       0.46 kB │ gzip:  0.30 kB\n",
                    project.replace('/', "-")));
                out.push_str("  ✓ built in 2.47s\n");
            }
        }
        out.push_str("\n");
    }

    out.push_str(&"─".repeat(60));
    out.push_str("\n\n");
    out.push_str(" NX   Failed to run target build for project apps/web\n\n");
    out.push_str("   Ran 20 tasks\n\n");
    out.push_str("   1 failed\n");
    out.push_str("   19 succeeded\n");
    out
}

// ─── New handler benchmarks ───────────────────────────────────────────────────

#[test]
fn benchmark_uv_install() {
    let input = uv_install_large();
    let in_tok = count_tokens(&input);

    let handler = UvHandler;
    let out = handler.filter(&input, &["uv".to_string(), "sync".to_string()]);
    let out_tok = count_tokens(&out);

    println!();
    println!("── uv sync (87 packages, large Django+ML project) ──");
    println!("{:<30} {:>12} {:>10} {:>10}", "Pass", "Without Panda", "With Panda", "Savings");
    println!("{}", "─".repeat(66));
    println!("{:<30} {:>12} {:>10} {:>9.0}%",
        "UvHandler", in_tok, out_tok, savings_pct(in_tok, out_tok));
    println!();
    println!("Output:\n{}", out);

    assert!(out.contains("Installed"), "install summary must be present");
    assert!(!out.contains("Downloading"), "download lines must be stripped");
    assert!(!out.contains("CPython"), "python version line must be stripped");
    assert!(
        savings_pct(in_tok, out_tok) >= 85.0,
        "expected ≥85% savings on uv sync output, got {:.0}%",
        savings_pct(in_tok, out_tok)
    );
}

#[test]
fn benchmark_ruff_check() {
    let input = ruff_check_large();
    let in_tok = count_tokens(&input);

    let handler = RuffHandler;
    let out = handler.filter(&input, &["ruff".to_string(), "check".to_string()]);
    let out_tok = count_tokens(&out);

    println!();
    println!("── ruff check (10 files, 100 violations, 8 distinct codes) ──");
    println!("{:<30} {:>12} {:>10} {:>10}", "Pass", "Without Panda", "With Panda", "Savings");
    println!("{}", "─".repeat(66));
    println!("{:<30} {:>12} {:>10} {:>9.0}%",
        "RuffHandler", in_tok, out_tok, savings_pct(in_tok, out_tok));
    println!();
    println!("Output:\n{}", out);

    assert!(out.contains("E501"), "E501 violations must be present");
    assert!(out.contains("F401"), "F401 violations must be present");
    assert!(
        savings_pct(in_tok, out_tok) >= 50.0,
        "expected ≥50% savings on ruff output, got {:.0}%",
        savings_pct(in_tok, out_tok)
    );
}

#[test]
fn benchmark_mypy() {
    let input = mypy_large();
    let in_tok = count_tokens(&input);

    let handler = MypyHandler;
    let out = handler.filter(&input, &[]);
    let out_tok = count_tokens(&out);

    println!();
    println!("── mypy (8 files, 60 errors, notes interleaved, daemon startup) ──");
    println!("{:<30} {:>12} {:>10} {:>10}", "Pass", "Without Panda", "With Panda", "Savings");
    println!("{}", "─".repeat(66));
    println!("{:<30} {:>12} {:>10} {:>9.0}%",
        "MypyHandler", in_tok, out_tok, savings_pct(in_tok, out_tok));
    println!();
    println!("Output:\n{}", out);

    assert!(!out.contains("Daemon"), "daemon startup line must be stripped");
    assert!(!out.contains(": note:"), "note lines must be stripped");
    assert!(out.contains("Found"), "summary line must be present");
    assert!(out.contains("more errors"), "per-file cap must be applied");
    assert!(
        savings_pct(in_tok, out_tok) >= 40.0,
        "expected ≥40% savings on mypy output, got {:.0}%",
        savings_pct(in_tok, out_tok)
    );
}

#[test]
fn benchmark_nx_run_many() {
    let input = nx_run_many_large();
    let in_tok = count_tokens(&input);

    let handler = NxHandler;
    let out = handler.filter(&input, &["nx".to_string(), "run-many".to_string()]);
    let out_tok = count_tokens(&out);

    println!();
    println!("── nx run-many (20 projects, 1 failing, 10 cached) ──");
    println!("{:<30} {:>12} {:>10} {:>10}", "Pass", "Without Panda", "With Panda", "Savings");
    println!("{}", "─".repeat(66));
    println!("{:<30} {:>12} {:>10} {:>9.0}%",
        "NxHandler", in_tok, out_tok, savings_pct(in_tok, out_tok));
    println!();
    println!("Output:\n{}", out);

    assert!(!out.contains("Nx Cloud"), "Nx Cloud banner must be stripped");
    assert!(out.contains("apps/web:build"), "failing task must be preserved");
    assert!(out.contains("RollupError"), "error details must be preserved");
    assert!(out.contains("cached"), "cache stats must be present");
    assert!(
        savings_pct(in_tok, out_tok) >= 70.0,
        "expected ≥70% savings on nx run-many output, got {:.0}%",
        savings_pct(in_tok, out_tok)
    );
}
