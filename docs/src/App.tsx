import { useState, useEffect, useRef } from 'react'
import {
  BarChart, Bar, XAxis, YAxis, CartesianGrid, Tooltip, Legend,
  PieChart, Pie, Cell, ResponsiveContainer,
  RadarChart, Radar, PolarGrid, PolarAngleAxis, PolarRadiusAxis,
} from 'recharts'

// ─── Theme ────────────────────────────────────────────────────────────────────
const T = {
  bg:         '#0f1117',
  sidebar:    '#13161f',
  card:       '#161b27',
  border:     '#1e2a3a',
  borderSoft: '#192236',
  code:       '#0d1117',
  text:       '#dce5f5',
  sub:        '#7a93b4',
  muted:      '#3d5270',
  cyan:       '#22d3ee',
  violet:     '#a78bfa',
  emerald:    '#34d399',
  amber:      '#f59e0b',
  red:        '#ef4444',
  indigo:     '#6366f1',
  link:       '#38bdf8',
}

// ─── Data ─────────────────────────────────────────────────────────────────────
const tokenTable = [
  { cmd: 'pip install',     before: 1787,  after: 9,    pct: 99 },
  { cmd: 'uv sync',         before: 1574,  after: 15,   pct: 99 },
  { cmd: 'playwright test', before: 1367,  after: 19,   pct: 99 },
  { cmd: 'docker build',    before: 1801,  after: 24,   pct: 99 },
  { cmd: 'swift build',     before: 1218,  after: 9,    pct: 99 },
  { cmd: 'dotnet build',    before: 438,   after: 3,    pct: 99 },
  { cmd: 'cmake',           before: 850,   after: 5,    pct: 99 },
  { cmd: 'gradle build',    before: 803,   after: 17,   pct: 98 },
  { cmd: 'go test',         before: 4507,  after: 148,  pct: 97 },
  { cmd: 'git merge',       before: 164,   after: 5,    pct: 97 },
  { cmd: 'pytest',          before: 3818,  after: 162,  pct: 96 },
  { cmd: 'terraform plan',  before: 3926,  after: 163,  pct: 96 },
  { cmd: 'npm install',     before: 648,   after: 25,   pct: 96 },
  { cmd: 'ember build',     before: 3377,  after: 139,  pct: 96 },
  { cmd: 'cargo build',     before: 1923,  after: 93,   pct: 95 },
  { cmd: 'cargo test',      before: 2782,  after: 174,  pct: 94 },
  { cmd: 'git clone',       before: 139,   after: 8,    pct: 94 },
  { cmd: 'bazel build',     before: 150,   after: 12,   pct: 92 },
  { cmd: 'next build',      before: 549,   after: 53,   pct: 90 },
  { cmd: 'cargo clippy',    before: 786,   after: 93,   pct: 88 },
  { cmd: 'make',            before: 545,   after: 72,   pct: 87 },
  { cmd: 'git diff',        before: 6370,  after: 861,  pct: 86 },
  { cmd: 'git push',        before: 173,   after: 24,   pct: 86 },
  { cmd: 'ls',              before: 691,   after: 102,  pct: 85 },
  { cmd: 'webpack',         before: 882,   after: 143,  pct: 84 },
  { cmd: 'vitest',          before: 625,   after: 103,  pct: 84 },
  { cmd: 'nx run-many',     before: 1541,  after: 273,  pct: 82 },
  { cmd: 'turbo run build', before: 597,   after: 115,  pct: 81 },
  { cmd: 'ruff check',      before: 2035,  after: 435,  pct: 79 },
  { cmd: 'eslint',          before: 4393,  after: 974,  pct: 78 },
  { cmd: 'grep',            before: 2925,  after: 691,  pct: 76 },
  { cmd: 'helm install',    before: 224,   after: 54,   pct: 76 },
  { cmd: 'docker ps',       before: 1057,  after: 266,  pct: 75 },
  { cmd: 'golangci-lint',   before: 3678,  after: 960,  pct: 74 },
  { cmd: 'git log',         before: 1573,  after: 431,  pct: 73 },
  { cmd: 'git status',      before: 650,   after: 184,  pct: 72 },
  { cmd: 'kubectl get pods',before: 2306,  after: 689,  pct: 70 },
  { cmd: 'vite build',      before: 526,   after: 182,  pct: 65 },
  { cmd: 'jest',            before: 330,   after: 114,  pct: 65 },
  { cmd: 'env',             before: 1155,  after: 399,  pct: 65 },
  { cmd: 'mvn install',     before: 4585,  after: 1613, pct: 65 },
  { cmd: 'brew install',    before: 368,   after: 148,  pct: 60 },
  { cmd: 'gh pr list',      before: 774,   after: 321,  pct: 59 },
  { cmd: 'biome lint',      before: 1503,  after: 753,  pct: 50 },
  { cmd: 'tsc',             before: 2598,  after: 1320, pct: 49 },
  { cmd: 'mypy',            before: 2053,  after: 1088, pct: 47 },
  { cmd: 'stylelint',       before: 1100,  after: 845,  pct: 23 },
]

const retrievalData = [
  { name: 'Basic search', score: 65.6, fill: T.indigo },
  { name: 'Panda',        score: 85.6, fill: T.cyan   },
]

const perRepo = [
  { repo: 'express',    basic: 80,  panda: 100 },
  { repo: 'flask',      basic: 100, panda: 100 },
  { repo: 'gin',        basic: 80,  panda: 100 },
  { repo: 'spring',     basic: 40,  panda: 80  },
  { repo: 'rails',      basic: 60,  panda: 80  },
  { repo: 'axios',      basic: 60,  panda: 80  },
  { repo: 'rust-anal.', basic: 60,  panda: 80  },
  { repo: 'abseil',     basic: 80,  panda: 100 },
  { repo: 'serilog',    basic: 60,  panda: 80  },
  { repo: 'riverpod',   basic: 60,  panda: 80  },
  { repo: 'okhttp',     basic: 80,  panda: 100 },
  { repo: 'laravel',    basic: 60,  panda: 80  },
  { repo: 'akka',       basic: 40,  panda: 60  },
  { repo: 'vapor',      basic: 60,  panda: 80  },
  { repo: 'vue-core',   basic: 80,  panda: 100 },
  { repo: 'svelte',     basic: 60,  panda: 80  },
  { repo: 'fastify',    basic: 60,  panda: 80  },
  { repo: 'fastapi',    basic: 60,  panda: 80  },
]

const ecosystemRadar = [
  { subject: 'JS / TS',      basic: 70, panda: 93  },
  { subject: 'Python',       basic: 80, panda: 90  },
  { subject: 'Go',           basic: 80, panda: 100 },
  { subject: 'Java/Kotlin',  basic: 40, panda: 70  },
  { subject: 'Rust',         basic: 60, panda: 80  },
  { subject: 'Ruby',         basic: 60, panda: 80  },
  { subject: 'Swift/Dart',   basic: 60, panda: 80  },
  { subject: 'C# / .NET',    basic: 60, panda: 80  },
]

const donutData = [
  { name: 'Saved', value: 67535, fill: T.cyan   },
  { name: 'Kept',  value: 14347, fill: T.border },
]

const tt = {
  contentStyle: { background: T.card, border: `1px solid ${T.border}`, borderRadius: 8, color: T.text, fontSize: 12 },
  labelStyle:   { color: T.sub },
  cursor:       { fill: 'rgba(255,255,255,0.03)' },
}

// ─── Nav structure ────────────────────────────────────────────────────────────
const NAV = [
  { group: 'Getting Started', items: [
    { id: 'overview',    label: 'Overview'         },
    { id: 'install',     label: 'Install'          },
    { id: 'quick-start', label: 'Quick start'      },
  ]},
  { group: 'How It Works', items: [
    { id: 'agents',      label: 'Supported agents'   },
    { id: 'pipeline',    label: 'Filtering pipeline' },
    { id: 'bert',        label: 'BERT engine'        },
    { id: 'handlers',    label: 'Handlers'           },
    { id: 'focus',       label: 'Context focusing'   },
  ]},
  { group: 'Benchmarks', items: [
    { id: 'token-savings',   label: 'Token savings'   },
    { id: 'file-retrieval',  label: 'File retrieval'  },
  ]},
  { group: 'Reference', items: [
    { id: 'commands',   label: 'Commands'      },
    { id: 'config',     label: 'Configuration' },
  ]},
  { group: 'More', items: [
    { id: 'security',     label: 'Privacy & security' },
    { id: 'faq',          label: 'FAQ'                },
    { id: 'contributing', label: 'Contributing'        },
  ]},
]

const TOC_ITEMS = [
  { id: 'overview',       label: 'Overview'             },
  { id: 'install',        label: 'Install'               },
  { id: 'quick-start',    label: 'Quick start'           },
  { id: 'agents',         label: 'Supported agents'      },
  { id: 'pipeline',       label: 'Filtering pipeline'    },
  { id: 'bert',           label: 'BERT engine'           },
  { id: 'handlers',       label: 'Handlers'              },
  { id: 'focus',          label: 'Context focusing'      },
  { id: 'token-savings',  label: 'Token savings'         },
  { id: 'file-retrieval', label: 'File retrieval'        },
  { id: 'commands',       label: 'Commands'              },
  { id: 'config',         label: 'Configuration'         },
  { id: 'security',       label: 'Privacy & security'    },
  { id: 'faq',            label: 'FAQ'                   },
  { id: 'contributing',   label: 'Contributing'          },
]

// ─── Shared UI components ─────────────────────────────────────────────────────

function H2({ id, children }: { id: string; children: React.ReactNode }) {
  return (
    <h2 id={id} style={{
      fontSize: 22, fontWeight: 700, color: T.text, marginTop: 56, marginBottom: 16,
      paddingTop: 24, borderTop: `1px solid ${T.border}`,
      scrollMarginTop: 80,
    }}>
      {children}
    </h2>
  )
}

function H3({ id, children }: { id?: string; children: React.ReactNode }) {
  return (
    <h3 id={id} style={{
      fontSize: 16, fontWeight: 600, color: T.text, marginTop: 32, marginBottom: 10,
      scrollMarginTop: 80,
    }}>
      {children}
    </h3>
  )
}

function P({ children }: { children: React.ReactNode }) {
  return <p style={{ fontSize: 14.5, color: T.sub, lineHeight: 1.8, marginBottom: 16 }}>{children}</p>
}

function Code({ children }: { children: React.ReactNode }) {
  return (
    <code style={{
      fontFamily: 'JetBrains Mono, Menlo, monospace',
      fontSize: 12.5, background: T.code, color: T.cyan,
      padding: '2px 6px', borderRadius: 4, border: `1px solid ${T.border}`,
    }}>
      {children}
    </code>
  )
}

function CodeBlock({ lang = 'bash', children }: { lang?: string; children: string }) {
  return (
    <div style={{
      background: T.code, border: `1px solid ${T.border}`, borderRadius: 10,
      marginBottom: 20, overflow: 'hidden',
    }}>
      <div style={{
        padding: '8px 16px', background: T.sidebar,
        borderBottom: `1px solid ${T.border}`,
        fontSize: 11, color: T.muted, fontFamily: 'monospace',
      }}>
        {lang}
      </div>
      <pre style={{
        padding: '18px 20px', margin: 0, overflowX: 'auto',
        fontFamily: 'JetBrains Mono, Menlo, monospace',
        fontSize: 13, lineHeight: 1.7, color: T.text,
      }}>
        {children.trim()}
      </pre>
    </div>
  )
}

function Callout({ type = 'tip', children }: { type?: 'tip' | 'note' | 'warning'; children: React.ReactNode }) {
  const cfg = {
    tip:     { color: T.cyan,    label: 'TIP',     bg: 'rgba(34,211,238,0.05)'  },
    note:    { color: T.violet,  label: 'NOTE',    bg: 'rgba(167,139,250,0.05)' },
    warning: { color: T.amber,   label: 'WARNING', bg: 'rgba(245,158,11,0.05)'  },
  }[type]
  return (
    <div style={{
      borderLeft: `3px solid ${cfg.color}`, background: cfg.bg,
      borderRadius: '0 8px 8px 0', padding: '14px 18px', marginBottom: 20,
    }}>
      <span style={{ fontSize: 11, fontWeight: 700, color: cfg.color, letterSpacing: '0.06em', display: 'block', marginBottom: 6 }}>
        {cfg.label}
      </span>
      <div style={{ fontSize: 14, color: T.sub, lineHeight: 1.7 }}>{children}</div>
    </div>
  )
}

function StatRow({ items }: { items: { value: string; label: string; color?: string }[] }) {
  return (
    <div style={{
      display: 'grid', gridTemplateColumns: `repeat(${items.length}, 1fr)`,
      gap: 1, background: T.border, borderRadius: 10, overflow: 'hidden',
      marginBottom: 28,
    }}>
      {items.map(({ value, label, color = T.cyan }) => (
        <div key={label} style={{ background: T.card, padding: '20px 24px', textAlign: 'center' }}>
          <div style={{ fontSize: 30, fontWeight: 900, color, fontVariantNumeric: 'tabular-nums', lineHeight: 1 }}>{value}</div>
          <div style={{ fontSize: 12, color: T.sub, marginTop: 6, lineHeight: 1.4 }}>{label}</div>
        </div>
      ))}
    </div>
  )
}

function DataTable({ headers, rows, highlight }: {
  headers: string[]
  rows: (string | number)[][]
  highlight?: number
}) {
  return (
    <div style={{
      background: T.card, border: `1px solid ${T.border}`, borderRadius: 10,
      overflow: 'hidden', marginBottom: 28,
    }}>
      <table style={{ width: '100%', borderCollapse: 'collapse', fontSize: 13 }}>
        <thead>
          <tr style={{ background: T.sidebar }}>
            {headers.map((h, i) => (
              <th key={i} style={{
                padding: '10px 16px', textAlign: i === 0 ? 'left' : 'right',
                fontWeight: 600, color: T.muted, fontSize: 11, letterSpacing: '0.05em',
                textTransform: 'uppercase', borderBottom: `1px solid ${T.border}`,
              }}>
                {h}
              </th>
            ))}
          </tr>
        </thead>
        <tbody>
          {rows.map((row, ri) => (
            <tr key={ri} style={{
              borderBottom: ri < rows.length - 1 ? `1px solid ${T.borderSoft}` : 'none',
              background: ri === rows.length - 1 && highlight !== undefined ? T.sidebar : 'transparent',
            }}>
              {row.map((cell, ci) => (
                <td key={ci} style={{
                  padding: '9px 16px',
                  textAlign: ci === 0 ? 'left' : 'right',
                  color: ci === 0 ? T.text : (typeof cell === 'string' && cell.startsWith('−') ? T.cyan : T.sub),
                  fontFamily: ci > 0 ? 'JetBrains Mono, monospace' : 'inherit',
                  fontWeight: ri === rows.length - 1 ? 700 : 400,
                }}>
                  {cell}
                </td>
              ))}
            </tr>
          ))}
        </tbody>
      </table>
    </div>
  )
}

function PageNav({ page, total, perPage, onPage }: {
  page: number; total: number; perPage: number; onPage: (p: number) => void
}) {
  const pages = Math.ceil(total / perPage)
  if (pages <= 1) return null
  const btn = (disabled: boolean): React.CSSProperties => ({
    background: 'none', border: `1px solid ${disabled ? T.borderSoft : T.border}`,
    borderRadius: 6, color: disabled ? T.muted : T.sub, cursor: disabled ? 'default' : 'pointer',
    padding: '5px 12px', fontSize: 12, fontFamily: 'inherit', lineHeight: 1,
  })
  return (
    <div style={{ display: 'flex', alignItems: 'center', gap: 10, justifyContent: 'flex-end', marginTop: 10 }}>
      <button style={btn(page === 0)} disabled={page === 0} onClick={() => onPage(page - 1)}>← Prev</button>
      <span style={{ fontSize: 12, color: T.muted, minWidth: 52, textAlign: 'center' }}>
        {page + 1} / {pages}
      </span>
      <button style={btn(page >= pages - 1)} disabled={page >= pages - 1} onClick={() => onPage(page + 1)}>Next →</button>
    </div>
  )
}

// ─── Doc sections ─────────────────────────────────────────────────────────────

function OverviewDiagram() {
  const W = 780, H = 212
  const c = T.cyan, red = T.red, b = T.border, m = T.muted, s = T.sub
  const code = '#0d1117', sb = T.sidebar

  const noiseLines = [
    'Compiling proc-macro2 v1.0.93',
    'Compiling syn v2.0.98',
    'Compiling tokio v1.44.1',
    'Compiling hyper v1.6.0',
    '... 46 more lines ...',
  ]

  const cleanLines = [
    { t: 'error[E0308]: mismatched types', col: red },
    { t: ' --> src/handler.rs:57:5',        col: m   },
    { t: '57|  Ok(value)',                  col: s   },
    { t: '   |     ^^^^^ expected &str',    col: red },
    { t: '[1 error — build failed]',         col: s   },
  ]

  return (
    <div style={{
      background: T.card, border: `1px solid ${T.border}`,
      borderRadius: 12, padding: '24px 20px 16px', marginBottom: 28,
    }}>
      <svg viewBox={`0 0 ${W} ${H}`} style={{ width: '100%', display: 'block' }}>
        <defs>
          <marker id="ov-c" markerWidth="8" markerHeight="6" refX="7" refY="3" orient="auto">
            <polygon points="0 0, 8 3, 0 6" fill={c}/>
          </marker>
          <marker id="ov-g" markerWidth="8" markerHeight="6" refX="7" refY="3" orient="auto">
            <polygon points="0 0, 8 3, 0 6" fill={m}/>
          </marker>
          <filter id="ov-glow" x="-60%" y="-60%" width="220%" height="220%">
            <feGaussianBlur in="SourceGraphic" stdDeviation="7" result="blur"/>
            <feMerge><feMergeNode in="blur"/><feMergeNode in="SourceGraphic"/></feMerge>
          </filter>
          <linearGradient id="ov-lg" x1="0" y1="0" x2="0" y2="1">
            <stop offset="0%" stopColor="#111827"/><stop offset="100%" stopColor={code}/>
          </linearGradient>
          <linearGradient id="ov-rg" x1="0" y1="0" x2="0" y2="1">
            <stop offset="0%" stopColor="#071a12"/><stop offset="100%" stopColor={code}/>
          </linearGradient>
        </defs>

        {/* ── LEFT BOX ─────────────────────────────── */}
        <rect x="8" y="38" width="188" height="132" rx="10" fill="url(#ov-lg)" stroke={b} strokeWidth="1.5"/>
        <rect x="8" y="38" width="188" height="32" rx="10" fill={sb}/>
        <rect x="8" y="54" width="188" height="16" fill={sb}/>
        <text x="102" y="58" textAnchor="middle" fill={s} fontSize="10.5" fontWeight="700" fontFamily="Inter,sans-serif" letterSpacing="0.06em">TOOL OUTPUT</text>

        {noiseLines.map((ln, i) => (
          <text key={i} x="16" y={84 + i * 14} fill="#4a6080" fontSize="9.5" fontFamily="JetBrains Mono,monospace">{ln}</text>
        ))}

        <rect x="34" y="152" width="128" height="14" rx="4" fill="rgba(239,68,68,0.1)" stroke="rgba(239,68,68,0.22)" strokeWidth="1"/>
        <text x="98" y="162" textAnchor="middle" fill={red} fontSize="9.5" fontWeight="700" fontFamily="Inter,sans-serif">1,923 tokens</text>

        {/* ── ARROW left → logo ──────────────────── */}
        <path d="M 196 104 L 326 104" stroke={m} strokeWidth="1.5" strokeDasharray="5 3" fill="none" markerEnd="url(#ov-g)"/>
        <text x="261" y="97" textAnchor="middle" fill={m} fontSize="9.5" fontFamily="Inter,sans-serif">raw output</text>

        {/* ── LOGO HUB ──────────────────────────── */}
        <circle cx="390" cy="104" r="72" fill="rgba(34,211,238,0.04)" stroke="rgba(34,211,238,0.13)" strokeWidth="1" filter="url(#ov-glow)"/>
        <circle cx="390" cy="104" r="58" fill={sb} stroke={c} strokeWidth="1.5"/>
        {/* logo 56×56, centered */}
        <image href={`${import.meta.env.BASE_URL}logo.png`} x="362" y="76" width="56" height="56"/>

        {/* ── ARROW logo → right ─────────────────── */}
        <path d="M 449 104 L 579 104" stroke={c} strokeWidth="2" fill="none" markerEnd="url(#ov-c)"/>
        <text x="514" y="97" textAnchor="middle" fill={c} fontSize="9.5" fontFamily="Inter,sans-serif" opacity="0.85">filtered</text>

        {/* ── RIGHT BOX ────────────────────────── */}
        <rect x="585" y="38" width="188" height="132" rx="10" fill="url(#ov-rg)" stroke={c} strokeWidth="1.5"/>
        <rect x="585" y="38" width="188" height="32" rx="10" fill={sb}/>
        <rect x="585" y="54" width="188" height="16" fill={sb}/>
        <text x="679" y="58" textAnchor="middle" fill={c} fontSize="10.5" fontWeight="700" fontFamily="Inter,sans-serif" letterSpacing="0.06em">AI AGENT SEES</text>

        {cleanLines.map(({ t, col }, i) => (
          <text key={i} x="593" y={84 + i * 14} fill={col} fontSize="9.5" fontFamily="JetBrains Mono,monospace">{t}</text>
        ))}

        <rect x="581" y="152" width="196" height="14" rx="4" fill="rgba(34,211,238,0.08)" stroke="rgba(34,211,238,0.22)" strokeWidth="1"/>
        <text x="679" y="162" textAnchor="middle" fill={c} fontSize="9.5" fontWeight="700" fontFamily="Inter,sans-serif">93 tokens · −95%</text>

        {/* ── LABELS BELOW LOGO ─────────────────── */}
        <text x="390" y="179" textAnchor="middle" fill={s} fontSize="11" fontWeight="600" fontFamily="Inter,sans-serif">PandaFilter</text>
        <text x="390" y="193" textAnchor="middle" fill={m} fontSize="9" fontFamily="Inter,sans-serif">on-device · all local</text>
      </svg>
    </div>
  )
}

function SectionOverview() {
  return (
    <>
      <div style={{
        background: 'linear-gradient(135deg, rgba(34,211,238,0.07) 0%, rgba(167,139,250,0.05) 100%)',
        border: `1px solid ${T.border}`, borderRadius: 12, padding: '32px 36px', marginBottom: 24,
      }}>
        <h1 style={{ fontSize: 28, fontWeight: 800, color: T.text, marginBottom: 12, lineHeight: 1.2 }}>
          PandaFilter
        </h1>
        <p style={{ fontSize: 16, color: T.sub, lineHeight: 1.7, marginBottom: 0, maxWidth: 620 }}>
          Cut your AI agent's token bill by 60–99% — by removing the noise from what it reads.
          PandaFilter intercepts build output, strips the garbage, and surfaces the right files.
          No config changes. Runs 100% locally.
        </p>
      </div>

      <OverviewDiagram />

      <StatRow items={[
        { value: '82%',   label: 'token reduction across 47 handlers', color: T.cyan   },
        { value: '85.6%', label: 'file retrieval success rate',        color: T.cyan   },
        { value: '59',    label: 'command handlers built-in',          color: T.violet },
        { value: '5',     label: 'AI agents supported',                color: T.emerald },
      ]} />

      <P>
        When your AI agent runs a command — <Code>pip install</Code>, <Code>cargo build</Code>, <Code>npm install</Code> — PandaFilter
        intercepts the output and removes everything the model doesn't need: download progress, module
        graphs, passing test lines, spinners. The agent sees a clean summary with errors, warnings,
        and results. Nothing useful is dropped.
      </P>

      <P>
        PandaFilter also maintains a file-relationship index for your repo. When you run a query,
        it uses hybrid BERT + lexical ranking to surface the most relevant files — so Claude reads
        what matters, not everything.
      </P>

      <Callout type="tip">
        Run <Code>panda gain</Code> after any session to see your cumulative token savings broken
        down by command.
      </Callout>
    </>
  )
}

function SectionInstall() {
  return (
    <>
      <H2 id="install">Install</H2>
      <H3>macOS (Homebrew)</H3>
      <CodeBlock lang="bash">{`
brew tap AssafWoo/pandafilter
brew install pandafilter
      `}</CodeBlock>

      <H3>Linux / any platform</H3>
      <CodeBlock lang="bash">{`
curl -fsSL https://raw.githubusercontent.com/AssafWoo/homebrew-pandafilter/main/install.sh | bash
      `}</CodeBlock>

      <Callout type="note">
        First run: PandaFilter downloads the BERT model (~90 MB, <Code>all-MiniLM-L6-v2</Code>) from
        HuggingFace and caches it at <Code>~/.cache/huggingface/</Code>. Subsequent runs are instant.
      </Callout>

      <H2 id="quick-start">Quick start</H2>
      <P>Wire PandaFilter into your AI agent with a single command:</P>
      <CodeBlock lang="bash">{`
panda init                         # Claude Code (default)
panda init --agent cursor          # Cursor
panda init --agent gemini          # Gemini CLI
panda init --agent cline           # Cline
panda init --agent copilot         # VS Code Copilot
      `}</CodeBlock>

      <P>
        That's it. From this point on, every command your agent runs is intercepted and filtered.
        Check that everything is wired correctly:
      </P>
      <CodeBlock lang="bash">{`
panda doctor          # diagnose the full installation
panda gain            # see token savings from this session
      `}</CodeBlock>

      <Callout type="tip">
        To see exactly what the agent received from a specific command:
        <br /><Code>panda run git log --oneline -20</Code>
      </Callout>
    </>
  )
}

function SectionAgents() {
  const agents = [
    {
      name: 'Claude Code',
      cmd: 'panda init',
      color: T.cyan,
      config: '~/.claude/settings.json',
      script: '~/.claude/hooks/panda-rewrite.sh',
      desc: 'Hooks wired into Claude\'s PreToolUse, PostToolUse, and UserPromptSubmit events.',
      logo: '◆',
    },
    {
      name: 'Cursor',
      cmd: 'panda init --agent cursor',
      color: T.violet,
      config: '~/.cursor/hooks.json',
      script: '~/.cursor/hooks/panda-rewrite.sh',
      desc: 'Same hook architecture as Claude Code, using Cursor\'s hooks.json config.',
      logo: '⬡',
    },
    {
      name: 'Gemini CLI',
      cmd: 'panda init --agent gemini',
      color: T.emerald,
      config: '~/.gemini/hooks.json',
      script: '~/.gemini/panda-rewrite.sh',
      desc: 'Hooks into Google\'s Gemini CLI tool-use pipeline via the gemini hooks config.',
      logo: '✦',
    },
    {
      name: 'Cline',
      cmd: 'panda init --agent cline',
      color: T.amber,
      config: '.clinerules (project dir)',
      script: '— (rules-based)',
      desc: 'Rules-based integration — injects filtering directives into the project\'s .clinerules file.',
      logo: '◈',
    },
    {
      name: 'VS Code Copilot',
      cmd: 'panda init --agent copilot',
      color: T.indigo,
      config: '.github/hooks/panda-rewrite.json',
      script: '.github/hooks/panda-rewrite.sh',
      desc: 'Stored per-project in .github/hooks — works with GitHub Copilot in VS Code.',
      logo: '◎',
    },
  ]

  const hookEvents = [
    {
      event: 'PreToolUse',
      what: 'Known handler → rewrites to panda run <cmd>. Unknown → no-op. Already wrapped → no double-wrap. Compound commands (;, &&, ||) → each segment rewritten independently.',
    },
    {
      event: 'PostToolUse',
      what: 'Bash → full 7-stage pipeline. Read → BERT + session dedup. Glob → paths grouped by directory. Grep → compact path format with match count.',
    },
    {
      event: 'UserPromptSubmit',
      what: 'Context Focusing module fires — queries the file index against the incoming prompt and injects guidance (recommended files, excluded files) before the model sees the message.',
    },
  ]

  return (
    <>
      <H2 id="agents">Supported agents</H2>
      <P>
        PandaFilter supports five AI coding agents. All share the same filtering binary and pipeline.
        Run <Code>panda init</Code> to wire it in — the command writes the necessary hooks and
        verifies the installation automatically.
      </P>

      <div style={{ display: 'grid', gridTemplateColumns: '1fr 1fr', gap: 12, marginBottom: 28 }}>
        {agents.map(({ name, cmd, color, config, script, desc, logo }) => (
          <div key={name} style={{
            background: T.card, border: `1px solid ${T.border}`, borderRadius: 10,
            padding: '18px 20px',
          }}>
            <div style={{ display: 'flex', alignItems: 'center', gap: 10, marginBottom: 10 }}>
              <span style={{ fontSize: 18, color, lineHeight: 1 }}>{logo}</span>
              <span style={{ fontSize: 14.5, fontWeight: 700, color: T.text }}>{name}</span>
            </div>
            <p style={{ fontSize: 12.5, color: T.sub, lineHeight: 1.6, marginBottom: 12 }}>{desc}</p>
            <div style={{
              background: T.code, borderRadius: 6, padding: '7px 12px',
              fontFamily: 'JetBrains Mono, monospace', fontSize: 12, color: T.cyan,
              marginBottom: 10,
            }}>
              {cmd}
            </div>
            <div style={{ display: 'flex', flexDirection: 'column', gap: 4 }}>
              <div style={{ fontSize: 11, color: T.muted }}>
                <span style={{ color: T.sub, fontWeight: 600 }}>Config: </span>{config}
              </div>
              <div style={{ fontSize: 11, color: T.muted }}>
                <span style={{ color: T.sub, fontWeight: 600 }}>Script: </span>{script}
              </div>
            </div>
          </div>
        ))}
        {/* Intentional empty cell to keep grid balanced for odd count */}
        <div />
      </div>

      <H3>Hook events</H3>
      <P>
        All agents share the same three hook events. Each fires at a different point in the agent's
        request lifecycle.
      </P>
      <div style={{ display: 'flex', flexDirection: 'column', gap: 10, marginBottom: 24 }}>
        {hookEvents.map(({ event, what }) => (
          <div key={event} style={{
            background: T.card, border: `1px solid ${T.border}`, borderRadius: 10,
            padding: '14px 18px', display: 'flex', gap: 14, alignItems: 'flex-start',
          }}>
            <code style={{
              flexShrink: 0, fontSize: 11.5, fontWeight: 700, color: T.cyan,
              fontFamily: 'JetBrains Mono, monospace', background: 'rgba(34,211,238,0.08)',
              padding: '3px 8px', borderRadius: 5, marginTop: 1,
              whiteSpace: 'nowrap',
            }}>{event}</code>
            <p style={{ fontSize: 13.5, color: T.sub, lineHeight: 1.65, margin: 0 }}>{what}</p>
          </div>
        ))}
      </div>

      <Callout type="tip">
        Hook integrity is enforced automatically. <Code>panda init</Code> writes SHA-256 baselines
        for the installed scripts (chmod 0o444). PandaFilter verifies them at every invocation and
        exits with a warning if tampered. Run <Code>panda verify</Code> to check all installed agents at once.
      </Callout>
    </>
  )
}

function SectionPipeline() {
  return (
    <>
      <H2 id="pipeline">Filtering pipeline</H2>
      <P>
        Every command output passes through a deterministic 7-stage pipeline before reaching the agent.
        Outputs under 15 tokens skip the pipeline entirely.
      </P>

      <div style={{
        background: T.card, border: `1px solid ${T.border}`, borderRadius: 10,
        padding: '24px 28px', marginBottom: 24,
      }}>
        {[
          ['0', 'Hard input ceiling', '200k chars — truncated before any stage'],
          ['1', 'Strip ANSI codes',   'Remove color codes and terminal escape sequences'],
          ['2', 'Normalize whitespace', 'Collapse blank lines, trim trailing space'],
          ['3', 'Global regex pre-filter', 'Progress bars, spinners, download lines, decorators'],
          ['4', 'NDJSON streaming compaction', 'go test -json, jest JSON reporter'],
          ['5', 'Command-specific filter', 'Handler-defined rules for each tool'],
          ['6', 'Entropy-adaptive BERT summarization', 'Up to 7 passes; falls back to head+tail'],
          ['7', 'Hard output cap', '50k chars'],
        ].map(([step, title, desc]) => (
          <div key={step} style={{ display: 'flex', gap: 16, marginBottom: 14, alignItems: 'flex-start' }}>
            <span style={{
              flexShrink: 0, width: 24, height: 24, borderRadius: 6,
              background: T.sidebar, border: `1px solid ${T.border}`,
              display: 'flex', alignItems: 'center', justifyContent: 'center',
              fontSize: 11, fontWeight: 700, color: T.cyan,
              fontFamily: 'monospace',
            }}>{step}</span>
            <div>
              <span style={{ fontSize: 14, fontWeight: 600, color: T.text }}>{title}</span>
              <span style={{ fontSize: 13, color: T.sub, marginLeft: 8 }}>— {desc}</span>
            </div>
          </div>
        ))}
      </div>

      <Callout type="note">
        <strong style={{ color: T.text }}>Pre-run cache</strong> — git, kubectl, docker, and terraform commands
        are hashed against live state before execution. A cache hit skips execution entirely and returns
        the cached output with a <Code>[PC: cached from Xm ago]</Code> marker.
      </Callout>
    </>
  )
}

function SectionBert() {
  const useCases = [
    {
      role: 'Output summarization',
      when: 'Pipeline step 6 — when command output exceeds the summarize threshold',
      what: 'Each output line is embedded and scored for anomaly (distance from the batch centroid). Lines that diverge most from the average are kept; repetitive noise is dropped.',
    },
    {
      role: 'Noise classification',
      when: 'Pipeline pre-filter — before summarization on large outputs',
      what: 'Lines are scored against two prototype embeddings ("useful" and "noise"). Lines with score < −0.05 are removed before the summarizer even runs.',
    },
    {
      role: 'File retrieval',
      when: 'At index time and query time in Context Focusing',
      what: 'Source file signatures are embedded at index time. At query time, the prompt is embedded and files are ranked by cosine similarity plus co-change and read-history signals.',
    },
    {
      role: 'BERT routing',
      when: 'When an unknown command is encountered',
      what: 'The command name is embedded and compared to stored handler embeddings. The closest match above a similarity threshold handles the output.',
    },
  ]

  return (
    <>
      <H2 id="bert">BERT engine</H2>
      <P>
        PandaFilter uses a local BERT sentence embedding model for three distinct jobs: deciding
        which lines of command output are worth keeping, classifying noise before summarization, and
        ranking files by relevance to a query. Everything runs on-device — no API calls, no data leaving
        your machine.
      </P>

      <H3>Model</H3>
      <div style={{
        display: 'grid', gridTemplateColumns: '1fr 1fr', gap: 1,
        background: T.border, borderRadius: 10, overflow: 'hidden', marginBottom: 24,
      }}>
        {[
          { label: 'Default model',   value: 'all-MiniLM-L6-v2' },
          { label: 'Alternative',     value: 'all-MiniLM-L12-v2' },
          { label: 'Embedding size',  value: '384 dimensions' },
          { label: 'Download size',   value: '~90 MB (one-time)' },
          { label: 'Cache location',  value: '~/.local/share/ccr/fastembed' },
          { label: 'Rust library',    value: 'fastembed crate' },
        ].map(({ label, value }) => (
          <div key={label} style={{ background: T.card, padding: '14px 20px' }}>
            <div style={{ fontSize: 11, color: T.muted, marginBottom: 4, textTransform: 'uppercase', letterSpacing: '0.05em' }}>{label}</div>
            <div style={{ fontSize: 13.5, fontFamily: 'JetBrains Mono, monospace', color: T.cyan }}>{value}</div>
          </div>
        ))}
      </div>

      <Callout type="note">
        The model is downloaded once on first run and cached at <Code>~/.local/share/ccr/fastembed</Code>.
        A sentinel file (<Code>~/.local/share/ccr/.bert_ready</Code>) is written after a successful
        download so subsequent runs skip the check entirely.
      </Callout>

      <H3>Where BERT is used</H3>
      <P>The model is used in four distinct places in the pipeline:</P>
      <div style={{ display: 'flex', flexDirection: 'column', gap: 10, marginBottom: 28 }}>
        {useCases.map(({ role, when, what }) => (
          <div key={role} style={{
            background: T.card, border: `1px solid ${T.border}`, borderRadius: 10, padding: '16px 20px',
          }}>
            <div style={{ display: 'flex', alignItems: 'flex-start', gap: 12 }}>
              <span style={{
                flexShrink: 0, marginTop: 2, width: 8, height: 8, borderRadius: '50%',
                background: T.cyan, display: 'inline-block',
              }} />
              <div>
                <div style={{ fontSize: 14, fontWeight: 600, color: T.text, marginBottom: 4 }}>{role}</div>
                <div style={{ fontSize: 12, color: T.muted, marginBottom: 6, fontStyle: 'italic' }}>{when}</div>
                <div style={{ fontSize: 13.5, color: T.sub, lineHeight: 1.6 }}>{what}</div>
              </div>
            </div>
          </div>
        ))}
      </div>

      <H3>Anomaly scoring</H3>
      <P>
        The core insight behind output summarization: in a wall of build output, interesting lines
        (errors, type mismatches, test failures) are statistically <em>unusual</em> — they differ
        from the bulk of the output. PandaFilter exploits this by measuring each line's distance from
        the batch centroid.
      </P>

      <div style={{
        background: T.code, border: `1px solid ${T.border}`, borderRadius: 10,
        padding: '20px 24px', marginBottom: 20, fontFamily: 'JetBrains Mono, monospace',
      }}>
        {[
          { label: 'centroid',      expr: 'mean of all line embeddings (L2-normalized)',               color: T.sub   },
          { label: 'anomaly(line)', expr: '1.0 − cosine_similarity(line_embedding, centroid)',          color: T.cyan  },
          { label: 'score(line)',   expr: '0.5 × anomaly + 0.5 × cosine_similarity(line, query)',      color: T.cyan  },
          { label: 'threshold',     expr: 'max_score × 0.30  (query mode) / 0.40 (anomaly-only mode)', color: T.amber },
        ].map(({ label, expr, color }) => (
          <div key={label} style={{ display: 'flex', gap: 16, marginBottom: 10, alignItems: 'baseline' }}>
            <span style={{ width: 130, flexShrink: 0, fontSize: 12.5, color: T.violet, textAlign: 'right' }}>{label}</span>
            <span style={{ fontSize: 12, color: T.muted }}>= </span>
            <span style={{ fontSize: 12.5, color }}>{expr}</span>
          </div>
        ))}
      </div>

      <P>
        Lines with a score above the threshold are kept, up to the budget. Lines matching the critical
        pattern (<Code>error|warning|failed|panic|exception</Code>) are always kept regardless of score.
      </P>

      <H3>Noise classification</H3>
      <P>
        Before the summarizer runs, a zero-shot noise filter scores every line against two fixed
        prototype strings. No training required — the model already knows what "errors" and
        "downloading" look like.
      </P>

      <div style={{
        display: 'grid', gridTemplateColumns: '1fr 1fr', gap: 16, marginBottom: 20,
      }}>
        {[
          {
            label: 'Useful prototype',
            color: T.emerald,
            text: '"error message stack trace type mismatch test failure file path function signature warning"',
          },
          {
            label: 'Noise prototype',
            color: T.red,
            text: '"compiling downloading resolving fetching progress elapsed already up to date artifact"',
          },
        ].map(({ label, color, text }) => (
          <div key={label} style={{
            background: T.card, border: `1px solid ${T.border}`, borderRadius: 10, padding: '16px 18px',
          }}>
            <div style={{ fontSize: 11, fontWeight: 700, color, letterSpacing: '0.05em', textTransform: 'uppercase', marginBottom: 10 }}>{label}</div>
            <div style={{ fontSize: 12, fontFamily: 'JetBrains Mono, monospace', color: T.sub, lineHeight: 1.6 }}>{text}</div>
          </div>
        ))}
      </div>

      <div style={{
        background: T.code, border: `1px solid ${T.border}`, borderRadius: 10,
        padding: '16px 22px', marginBottom: 20, fontFamily: 'JetBrains Mono, monospace', fontSize: 12.5,
      }}>
        <div style={{ color: T.sub, marginBottom: 6 }}>// score per line:</div>
        <div style={{ color: T.text }}>
          noise_score = cosine_sim(line, <span style={{ color: T.emerald }}>useful</span>) − cosine_sim(line, <span style={{ color: T.red }}>noise</span>)
        </div>
        <div style={{ color: T.amber, marginTop: 8 }}>if noise_score &lt; −0.05 → drop the line</div>
      </div>

      <H3>Entropy-adaptive budget</H3>
      <P>
        Before summarizing, PandaFilter measures the <em>semantic diversity</em> of the output —
        how spread out the line embeddings are. Repetitive output (all "downloading…" lines) gets
        a drastically smaller budget than diverse output (mixed errors, warnings, and trace lines).
      </P>

      <div style={{
        background: T.card, border: `1px solid ${T.border}`, borderRadius: 10,
        padding: '20px 24px', marginBottom: 20,
      }}>
        <div style={{ display: 'flex', flexDirection: 'column', gap: 12 }}>
          {[
            { range: 'entropy < 0.10',          budget: '~5% of max budget', label: 'Uniform / repetitive output',  color: T.red    },
            { range: '0.10 ≤ entropy ≤ 0.35',   budget: 'Linear interpolation', label: 'Mixed content',            color: T.amber  },
            { range: 'entropy > 0.35',           budget: '100% of max budget',   label: 'Diverse / rich output',    color: T.emerald },
          ].map(({ range, budget, label, color }) => (
            <div key={range} style={{ display: 'flex', alignItems: 'center', gap: 16 }}>
              <code style={{ width: 200, flexShrink: 0, fontSize: 12, color, fontFamily: 'JetBrains Mono, monospace' }}>{range}</code>
              <div style={{ flex: 1, height: 6, background: T.border, borderRadius: 3, overflow: 'hidden' }}>
                <div style={{
                  height: '100%', borderRadius: 3, background: color,
                  width: color === T.red ? '5%' : color === T.amber ? '50%' : '100%',
                }} />
              </div>
              <span style={{ fontSize: 12.5, color: T.sub, width: 160, flexShrink: 0 }}>{budget}</span>
              <span style={{ fontSize: 12, color: T.muted }}>{label}</span>
            </div>
          ))}
        </div>
      </div>

      <Callout type="tip">
        Entropy is measured on a sample of up to 100 lines (evenly spaced) to avoid O(N²) cost on huge
        outputs. If embeddings were already computed by the noise filter, they are reused — no second
        BERT pass needed.
      </Callout>

      <H3>Weights at a glance</H3>
      <DataTable
        headers={['Where used', 'Signal', 'Weight', 'Notes']}
        rows={[
          ['Summarization',       'Anomaly (distance from centroid)',    '0.5×', 'Always active'],
          ['Summarization',       'Relevance to query',                  '0.5×', 'Active when query available'],
          ['Intent-aware mode',   'Command relevance',                   '0.3×', 'Blended with intent'],
          ['Intent-aware mode',   'User intent relevance',               '0.7×', 'From last agent message'],
          ['File retrieval',      'Semantic similarity',                 '0.5×', 'With read history'],
          ['File retrieval',      'Co-change frequency (log-norm)',      '0.2×', 'With read history'],
          ['File retrieval',      'Read-history boost',                  '0.3×', 'With read history'],
          ['File retrieval',      'Semantic similarity',                 '0.7×', 'Without read history'],
          ['File retrieval',      'Co-change frequency',                 '0.3×', 'Without read history'],
          ['Role multipliers',    'entry_point',                         '1.5×', 'Applied after scoring'],
          ['Role multipliers',    'persistence',                         '1.2×', 'Applied after scoring'],
          ['Role multipliers',    'state_manager',                       '1.1×', 'Applied after scoring'],
        ]}
      />
    </>
  )
}

function SectionHandlers() {
  const [page, setPage] = useState(0)
  const PER = 15
  const rows: [string, string, string][] = [
    ['cargo',          'cargo',                           'build/clippy: errors (capped at 15) + warning count. test: failures + summary. nextest run: FAIL lines + Summary.'],
    ['git',            'git',                             'status: counts. log: --oneline, cap 50. diff: 2 context lines, 200-line cap. clone/merge/checkout/rebase: compressed success or full conflict output.'],
    ['go',             'go',                              'test: NDJSON streaming, FAIL blocks + summary. build: errors only.'],
    ['ember',          'ember',                           'build: errors + summary; drops fingerprint/asset spam. test: failures + summary. serve: serving URL only.'],
    ['tsc',            'tsc',                             'Errors grouped by file; deduplicates repeated TS codes. Build OK on clean. Injects --noEmit.'],
    ['vitest',         'vitest',                          'FAIL blocks + summary; drops ✓ lines.'],
    ['jest',           'jest, bun, deno',                 '● failure blocks + summary; drops PASS lines.'],
    ['pytest',         'pytest',                          'FAILED node IDs + AssertionError + short summary. Injects --tb=short.'],
    ['rspec',          'rspec',                           'Injects --format json; example-level failures with message + location.'],
    ['rubocop',        'rubocop',                         'Injects --format json; offenses grouped by severity, capped.'],
    ['rake',           'rake, bundle',                    'Failure/error blocks + summary; drops passing test lines.'],
    ['mypy',           'mypy',                            'Errors grouped by file, capped at 10 per file. Injects --no-color.'],
    ['ruff',           'ruff',                            'Violations grouped by error code. format: summary line only.'],
    ['uv',             'uv, uvx',                         'Strips Downloading/Fetching/Preparing noise; keeps errors + summary.'],
    ['pip',            'pip, poetry, pdm, conda',         'install: [complete — N packages] or already-satisfied short-circuit.'],
    ['python',         'python',                          'Traceback: keep block + final error. Detects and compresses tabular/CSV, pandas DataFrames, Word, Excel, PowerPoint output. Long output: BERT.'],
    ['eslint',         'eslint',                          'Errors grouped by file, caps at 20 + [+N more].'],
    ['next',           'next',                            'build: route table collapsed. dev: errors + ready line.'],
    ['playwright',     'playwright',                      'Failing test names + error messages; passing tests dropped. Injects --reporter=list.'],
    ['prettier',       'prettier',                        '--check: files needing formatting + count.'],
    ['vite',           'vite',                            'Asset chunk table collapsed, HMR deduplication.'],
    ['webpack',        'webpack',                         'Module resolution graph dropped; keeps assets, errors, build result.'],
    ['turbo',          'turbo',                           'Inner task output stripped; cache hit/miss per package + final summary.'],
    ['nx',             'nx, npx nx',                      'Passing tasks collapsed to [N tasks passed]; failing task output kept.'],
    ['stylelint',      'stylelint',                       'Issues grouped by file, caps at 40 + [+N more].'],
    ['biome',          'biome',                           'Code context snippets stripped; keeps file:line, rule, message.'],
    ['kubectl',        'kubectl, k',                      'get pods: aggregates to [N pods, all running] or problem-pods table. Log anomaly scoring. describe key sections. events: warning-only, capped at 20.'],
    ['terraform',      'terraform, tofu',                 'plan: +/-/~ + summary. validate: short-circuits on success. output: compact key=value. state list: capped at 50.'],
    ['aws',            'aws, gcloud, az',                 'Resource extraction; --output json injected for read-only actions.'],
    ['gh',             'gh',                              'Compact tables for list commands; strips HTML from pr view.'],
    ['helm',           'helm',                            'list: compact table. status/diff/template: structured output.'],
    ['docker',         'docker',                          'logs: ANSI strip + BERT. ps/images: formatted tables + total size. build: errors + final image ID.'],
    ['make',           'make, ninja',                     '"Nothing to be done" short-circuit; keeps errors. Injects --no-print-directory.'],
    ['golangci-lint',  'golangci-lint',                   'Diagnostics grouped by file; runner noise dropped. Detects v1 text and v2 JSON formats.'],
    ['prisma',         'prisma',                          'generate/migrate/db push structured summaries.'],
    ['mvn',            'mvn',                             'Drops [INFO] noise; keeps errors + reactor summary.'],
    ['gradle',         'gradle',                          'UP-TO-DATE tasks collapsed; FAILED tasks and errors kept.'],
    ['npm/yarn',       'npm, yarn',                       'install: package count; strips boilerplate.'],
    ['pnpm',           'pnpm',                            'install: summary; drops progress bars.'],
    ['brew',           'brew',                            'install/update: status lines + Caveats.'],
    ['curl',           'curl',                            'JSON → type schema. Non-JSON: cap 30 lines.'],
    ['grep/rg',        'grep, rg',                        'Compact paths, per-file 100-match cap, line numbers preserved, [N matches in M files] summary. Injects --no-heading --with-filename. Match-centered line truncation.'],
    ['find',           'find',                            'Groups by directory, caps at 50. Injects -maxdepth 8 if unset.'],
    ['journalctl',     'journalctl',                      'Injects --no-pager -n 200. BERT anomaly scoring.'],
    ['psql',           'psql',                            'Strips borders, caps at 20 rows.'],
    ['tree',           'tree',                            'Auto-injects -I "node_modules|.git|target|…".'],
    ['diff',           'diff',                            '+/-/@@ + 2 context lines, max 5 hunks.'],
    ['jq',             'jq',                              'Array: schema of first element + [N items].'],
    ['env',            'env',                             'Categorized sections; sensitive values redacted.'],
    ['ls',             'ls',                              'Drops noise dirs; top-3 extension summary.'],
    ['log',            'log',                             'Timestamp/UUID normalization, dedup [×N], error summary block.'],
    ['rsync',          'rsync',                           'Drops per-file transfer progress lines (to-chk=, MB/s); keeps file list and final summary.'],
    ['ffmpeg',         'ffmpeg, ffprobe',                 'Drops frame= and size= real-time progress lines; keeps input/output codec info and final size line.'],
    ['wget',           'wget',                            'Injects --quiet if no verbosity flag set.'],
    ['swift',          'swift, swift-build, swift-test',  'build: errors/warnings + Build complete. test: failures + summary. package resolve: strips progress.'],
    ['dotnet',         'dotnet, dotnet-cli',              'build: errors grouped by CS code + summary. Short-circuits on clean build. test: failures + summary. restore: package count.'],
    ['cmake',          'cmake, cmake3',                   'configure: errors + final written-to line. --build: errors + [N targets built]. Auto-detects mode from args/output.'],
    ['bazel',          'bazel, bazelisk, bzl',            'build: errors + completion summary [N actions, build OK (Xs)]. test: failures + [N passed, N failed]. query: cap at 30 targets.'],
  ]
  return (
    <>
      <H2 id="handlers">Handlers</H2>
      <P>
        PandaFilter ships with <strong style={{ color: T.text }}>59 handlers</strong> covering 70+ command
        aliases. When an unknown command is encountered, BERT routing matches it to the closest handler
        by embedding similarity. If no match is found, output passes through unchanged.
      </P>
      <P>
        Lookup cascade: <strong style={{ color: T.text }}>user filters</strong> →{' '}
        <strong style={{ color: T.text }}>exact match</strong> →{' '}
        <strong style={{ color: T.text }}>static alias table</strong> →{' '}
        <strong style={{ color: T.text }}>BERT routing</strong>.
      </P>

      <div style={{
        background: T.card, border: `1px solid ${T.border}`, borderRadius: 10,
        overflow: 'hidden', marginBottom: 0,
      }}>
        <table style={{ width: '100%', borderCollapse: 'collapse', fontSize: 12.5 }}>
          <thead>
            <tr style={{ background: T.sidebar }}>
              {['Handler', 'Aliases / keys', 'Behavior'].map((h, i) => (
                <th key={i} style={{
                  padding: '10px 14px', textAlign: 'left',
                  fontWeight: 600, color: T.muted, fontSize: 11,
                  letterSpacing: '0.05em', textTransform: 'uppercase',
                  borderBottom: `1px solid ${T.border}`,
                  width: i === 0 ? 100 : i === 1 ? 170 : undefined,
                }}>
                  {h}
                </th>
              ))}
            </tr>
          </thead>
          <tbody>
            {rows.slice(page * PER, (page + 1) * PER).map(([handler, keys, desc], i) => (
              <tr key={i} style={{ borderBottom: `1px solid ${T.borderSoft}` }}>
                <td style={{ padding: '8px 14px', fontFamily: 'monospace', fontSize: 12, color: T.cyan, whiteSpace: 'nowrap', verticalAlign: 'top' }}>
                  {handler}
                </td>
                <td style={{ padding: '8px 14px', fontFamily: 'monospace', fontSize: 11, color: T.muted, verticalAlign: 'top' }}>
                  {keys}
                </td>
                <td style={{ padding: '8px 14px', color: T.sub, lineHeight: 1.55, verticalAlign: 'top' }}>{desc}</td>
              </tr>
            ))}
          </tbody>
        </table>
      </div>
      <PageNav page={page} total={rows.length} perPage={PER} onPage={setPage} />

      <div style={{ marginTop: 20 }}>
        <Callout type="note">
          To add a custom handler: implement the <Code>Handler</Code> trait and register it in <Code>ccr/src/handlers/mod.rs</Code> — see <Code>git.rs</Code> as a template.
        </Callout>
      </div>
    </>
  )
}

function SectionFocus() {
  return (
    <>
      <H2 id="focus">Context focusing</H2>
      <P>
        Context Focusing is an opt-in feature that tells the agent which files are relevant for the
        current prompt, preventing it from reading unrelated files. It builds a file-relationship index
        using BERT embeddings and co-change history from git, then surfaces the top files for each query.
      </P>

      <CodeBlock lang="bash">{`
panda focus --enable     # enable for this repo
panda focus --disable    # disable (keeps index data)
panda focus --status     # show status + index age
panda focus --dry-run    # preview guidance without enabling
panda index              # manually rebuild the index
      `}</CodeBlock>

      <Callout type="warning">
        Context Focusing is disabled by default. Run <Code>panda doctor</Code> to confirm the index
        is ready before enabling. Requires repos with at least 25 files and 2,000 source lines.
      </Callout>

      <H3>How the index works</H3>
      <P>
        At index time, PandaFilter extracts structural signatures (function names, struct definitions,
        type signatures) from every source file and encodes them into embeddings. Co-change frequency
        — how often two files are modified in the same commit — is recorded from git history.
      </P>
      <P>
        At query time, ranking is a weighted blend of:
      </P>

      <div style={{
        background: T.card, border: `1px solid ${T.border}`, borderRadius: 10,
        padding: '20px 24px', marginBottom: 24,
      }}>
        {[
          ['0.5', 'Semantic similarity', 'BERT embedding distance between query and file signatures'],
          ['0.2', 'Co-change frequency', 'Log-normalized frequency of co-modification with other touched files'],
          ['0.3', 'Read-history boost', 'Files you already read in this session get surfaced again'],
        ].map(([w, name, desc]) => (
          <div key={name} style={{ display: 'flex', gap: 16, marginBottom: 12, alignItems: 'flex-start' }}>
            <span style={{
              flexShrink: 0, padding: '2px 8px', borderRadius: 4,
              background: 'rgba(34,211,238,0.1)', color: T.cyan,
              fontSize: 12, fontWeight: 700, fontFamily: 'monospace',
            }}>{w}×</span>
            <div>
              <span style={{ fontSize: 14, fontWeight: 600, color: T.text }}>{name}</span>
              <span style={{ fontSize: 13, color: T.sub, display: 'block', marginTop: 2 }}>{desc}</span>
            </div>
          </div>
        ))}
      </div>

      <P>
        Entry-point files get a 1.5× role multiplier; persistence files get 1.2×; state managers 1.1×.
        On top of BERT ranking, a lexical pass matches query tokens against stored signatures
        (including camelCase expansion), giving a final hybrid score.
      </P>
    </>
  )
}

function SectionTokenSavings() {
  const [page, setPage] = useState(0)
  const PER = 15
  const dataRows = tokenTable.map(r => [
    r.cmd,
    r.before.toLocaleString(),
    r.after.toLocaleString(),
    `−${r.pct}%`,
  ])
  const totalRow = ['Total', '81,882', '14,347', '−82%']
  const isLast = page === Math.ceil(dataRows.length / PER) - 1
  const tableRows = [
    ...dataRows.slice(page * PER, (page + 1) * PER),
    ...(isLast ? [totalRow] : []),
  ]

  return (
    <>
      <H2 id="token-savings">Token savings</H2>
      <P>
        Numbers from <Code>ccr/tests/handler_benchmarks.rs</Code>. Measured against real command
        output, not synthetic data. Run <Code>panda gain</Code> to see your own live numbers.
      </P>

      <StatRow items={[
        { value: '81,882', label: 'tokens without Panda',   color: T.red    },
        { value: '14,347', label: 'tokens with Panda',      color: T.cyan   },
        { value: '67,535', label: 'tokens eliminated',      color: T.emerald },
        { value: '−82%',   label: 'overall reduction',      color: T.cyan   },
      ]} />

      <div style={{ display: 'grid', gridTemplateColumns: '220px 1fr', gap: 24, marginBottom: 28 }}>
        <div style={{
          background: T.card, border: `1px solid ${T.border}`, borderRadius: 10,
          padding: '24px 20px', display: 'flex', flexDirection: 'column', alignItems: 'center',
        }}>
          <p style={{ fontSize: 12, color: T.sub, marginBottom: 16, textAlign: 'center' }}>
            Share of tokens eliminated
          </p>
          <div style={{ position: 'relative' }}>
            <PieChart width={160} height={160}>
              <Pie data={donutData} cx={80} cy={80} innerRadius={50} outerRadius={70}
                dataKey="value" startAngle={90} endAngle={-270} strokeWidth={0}>
                {donutData.map((d, i) => <Cell key={i} fill={d.fill} />)}
              </Pie>
            </PieChart>
            <div style={{
              position: 'absolute', inset: 0, display: 'flex', flexDirection: 'column',
              alignItems: 'center', justifyContent: 'center', pointerEvents: 'none',
            }}>
              <span style={{ fontSize: 26, fontWeight: 900, color: T.cyan }}>82%</span>
            </div>
          </div>
        </div>

        <div style={{
          background: T.card, border: `1px solid ${T.border}`, borderRadius: 10, padding: '24px 28px',
        }}>
          <p style={{ fontSize: 12, color: T.sub, marginBottom: 20 }}>Without Panda vs With Panda — total tokens</p>
          <div style={{ display: 'flex', flexDirection: 'column', gap: 18 }}>
            {[
              { label: 'Without Panda', n: 81882, color: T.red },
              { label: 'With Panda',    n: 14347, color: T.cyan },
            ].map(({ label, n, color }) => (
              <div key={label}>
                <div style={{ display: 'flex', justifyContent: 'space-between', marginBottom: 8 }}>
                  <span style={{ fontSize: 13, color: T.sub }}>{label}</span>
                  <span style={{ fontSize: 13, fontWeight: 700, color, fontFamily: 'monospace' }}>{n.toLocaleString()}</span>
                </div>
                <div style={{ height: 24, background: T.border, borderRadius: 6, overflow: 'hidden' }}>
                  <div style={{
                    width: `${(n / 81882) * 100}%`, height: '100%', borderRadius: 6,
                    background: color, boxShadow: `0 0 10px ${color}44`,
                  }} />
                </div>
              </div>
            ))}
          </div>
        </div>
      </div>

      <H3>Per-command breakdown</H3>
      <DataTable
        headers={['Command', 'Without Panda', 'With Panda', 'Savings']}
        rows={tableRows}
        highlight={isLast ? tableRows.length - 1 : undefined}
      />
      <PageNav page={page} total={dataRows.length} perPage={PER} onPage={setPage} />
    </>
  )
}

function SectionRetrieval() {
  return (
    <>
      <H2 id="file-retrieval">File retrieval</H2>
      <P>
        We ran the same 90 retrieval tasks across the same 18 open-source repos used in published
        retrieval benchmarks, measuring <strong style={{ color: T.text }}>hit@5</strong> (whether the target file
        appears in the top 5 results) and <strong style={{ color: T.text }}>MRR@5</strong> (mean reciprocal rank).
      </P>

      <StatRow items={[
        { value: '85.6%', label: 'hit@5 — Panda hybrid ranking',  color: T.cyan   },
        { value: '65.6%', label: 'hit@5 — semantic search alone', color: T.indigo },
        { value: '+20pp', label: 'improvement from hybrid ranking', color: T.emerald },
        { value: '0.73',  label: 'MRR@5 (avg rank ≈ 1.4)',        color: T.cyan   },
      ]} />

      <H3>Retrieval quality</H3>
      <P>
        The chart below compares basic semantic-only search (BERT embedding similarity) against
        Panda's hybrid approach (BERT + lexical scoring on structural signatures).
      </P>

      <div style={{ display: 'grid', gridTemplateColumns: '1fr 1fr', gap: 20, marginBottom: 24 }}>
        <div style={{ background: T.card, border: `1px solid ${T.border}`, borderRadius: 10, padding: '20px 20px 12px' }}>
          <p style={{ fontSize: 12, color: T.sub, marginBottom: 16 }}>Hit@5 — correct file in top 5 results</p>
          <ResponsiveContainer width="100%" height={220}>
            <BarChart data={retrievalData} barCategoryGap="45%">
              <CartesianGrid strokeDasharray="3 3" stroke={T.border} />
              <XAxis dataKey="name" tick={{ fill: T.sub, fontSize: 11 }} />
              <YAxis domain={[0, 100]} tickFormatter={v => `${v}%`} tick={{ fill: T.sub, fontSize: 11 }} />
              <Tooltip {...tt} formatter={(v: any) => [`${v}%`, 'Hit@5']} />
              <Bar dataKey="score" radius={[5, 5, 0, 0]}>
                {retrievalData.map((d, i) => <Cell key={i} fill={d.fill} />)}
              </Bar>
            </BarChart>
          </ResponsiveContainer>
        </div>

        <div style={{ background: T.card, border: `1px solid ${T.border}`, borderRadius: 10, padding: '20px 20px 12px' }}>
          <p style={{ fontSize: 12, color: T.sub, marginBottom: 16 }}>Coverage by programming language</p>
          <ResponsiveContainer width="100%" height={220}>
            <RadarChart data={ecosystemRadar}>
              <PolarGrid stroke={T.border} />
              <PolarAngleAxis dataKey="subject" tick={{ fill: T.sub, fontSize: 10 }} />
              <PolarRadiusAxis domain={[0, 100]} tick={false} axisLine={false} />
              <Radar name="Basic" dataKey="basic" stroke={T.indigo} fill={T.indigo} fillOpacity={0.15} />
              <Radar name="Panda" dataKey="panda" stroke={T.cyan}   fill={T.cyan}   fillOpacity={0.20} />
              <Legend wrapperStyle={{ fontSize: 11, color: T.sub }} />
              <Tooltip {...tt} formatter={(v: any) => [`${v}%`]} />
            </RadarChart>
          </ResponsiveContainer>
        </div>
      </div>

      <H3>Results per repository</H3>
      <P>Each repo has 5 tasks. The chart shows the percentage answered correctly.</P>
      <div style={{ background: T.card, border: `1px solid ${T.border}`, borderRadius: 10, padding: '20px 20px 8px', marginBottom: 24 }}>
        <ResponsiveContainer width="100%" height={320}>
          <BarChart data={perRepo} barCategoryGap="22%" barGap={2}>
            <CartesianGrid strokeDasharray="3 3" stroke={T.border} />
            <XAxis dataKey="repo" tick={{ fill: T.sub, fontSize: 9 }} angle={-35} textAnchor="end" height={54} />
            <YAxis domain={[0, 100]} tickFormatter={v => `${v}%`} tick={{ fill: T.sub, fontSize: 10 }} />
            <Tooltip {...tt} formatter={(v: any) => [`${v}%`]} />
            <Legend wrapperStyle={{ color: T.sub, fontSize: 11, paddingTop: 10 }} />
            <Bar dataKey="basic" name="Basic search" fill={T.indigo} radius={[3, 3, 0, 0]} />
            <Bar dataKey="panda" name="Panda"         fill={T.cyan}   radius={[3, 3, 0, 0]} />
          </BarChart>
        </ResponsiveContainer>
      </div>

      <H3>Why hybrid ranking wins</H3>
      <P>
        Pure semantic search embeds file content but misses lexical signals — a query for
        "register route with url rules" may not score highly on <Code>app.py</Code> just from
        embeddings. The lexical pass scores exact token matches against extracted signatures
        (function names, type names), path components, and camelCase expansions. Together they
        cover what BERT misses.
      </P>
      <Callout type="note">
        The benchmark uses 10× BERT oversampling before lexical reranking to ensure files that
        rank 6–50 in embedding space still get a chance to surface via lexical scoring.
      </Callout>
    </>
  )
}

function SectionCommands() {
  return (
    <>
      <H2 id="commands">Commands</H2>

      <H3>panda gain</H3>
      <P>See your token savings from the current or recent sessions.</P>
      <CodeBlock lang="bash">{`
panda gain                    # overall summary
panda gain --breakdown        # per-command table
panda gain --history          # last 14 days
panda gain --insight          # categorized savings + top saves
      `}</CodeBlock>

      <H3>panda focus</H3>
      <P>Manage the file-relationship index and context focusing feature.</P>
      <CodeBlock lang="bash">{`
panda focus --enable          # enable for this repo
panda focus --disable         # disable (keeps index data)
panda focus --status          # show status + index age
panda focus --dry-run         # preview without enabling
panda index                   # manually rebuild the index
      `}</CodeBlock>

      <H3>panda doctor</H3>
      <P>Diagnose the full installation — hooks, BERT model, index health, agent wiring.</P>
      <CodeBlock lang="bash">{`
panda doctor
      `}</CodeBlock>

      <H3>Other commands</H3>
      <CodeBlock lang="bash">{`
panda verify                           # check hook integrity
panda discover                         # scan history for unfiltered commands
panda run git status                   # run a command through PandaFilter manually
panda proxy git status                 # run raw (no filtering), record baseline
panda read-file src/main.rs --level auto  # preview read filtering
panda expand ZI_3                      # restore a collapsed block
panda noise                            # show learned noise patterns
panda compress --scan-session          # compress current conversation context
      `}</CodeBlock>

      <H3>panda init --uninstall</H3>
      <CodeBlock lang="bash">{`
panda init --uninstall                  # Claude Code
panda init --agent cursor --uninstall  # Cursor
      `}</CodeBlock>
    </>
  )
}

function SectionConfig() {
  return (
    <>
      <H2 id="config">Configuration</H2>
      <P>
        Config is loaded in order: <Code>./panda.toml</Code> → <Code>~/.config/panda/config.toml</Code> → embedded default.
        All fields are optional and override the default.
      </P>

      <CodeBlock lang="toml">{`
[global]
summarize_threshold_lines = 50
head_lines = 30
tail_lines = 30
strip_ansi = true
normalize_whitespace = true
deduplicate_lines = true
input_char_ceiling = 200000
output_char_cap = 50000
# cost_per_million_tokens = 15.0

[tee]
enabled = true
mode = "aggressive"   # "aggressive" | "always" | "never"

[read]
mode = "auto"   # "passthrough" | "auto" | "strip" | "aggressive"

[focus]
enabled = false
min_files = 25
min_lines = 2000
      `}</CodeBlock>

      <H3>User-defined filters</H3>
      <P>
        Place <Code>filters.toml</Code> at <Code>.panda/filters.toml</Code> (project-local) or
        <Code>~/.config/panda/filters.toml</Code> (global). Project-local overrides global for the same key.
      </P>
      <CodeBlock lang="toml">{`
[commands.myapp]
patterns = [
  { regex = "^DEBUG:",            action = "Remove" },
  { regex = "^\\S+\\.ts\\(",     action = "TruncateLinesAt", max_chars = 120 },
]
on_empty = "(no relevant output)"

[commands.myapp.match_output]
pattern        = "Server started"
message        = "ok — server ready"
unless_pattern = "error"
      `}</CodeBlock>

      <P>
        Pattern actions: <Code>Remove</Code>, <Code>Collapse</Code>, <Code>ReplaceWith</Code>,{' '}
        <Code>TruncateLinesAt</Code>, <Code>HeadLines</Code>, <Code>TailLines</Code>,{' '}
        <Code>MatchOutput</Code>, <Code>OnEmpty</Code>.
      </P>
    </>
  )
}

function SecurityDiagram() {
  const W = 780, H = 258

  // named colours used inside SVG (can't reference T.* directly in SVG attributes without interpolation)
  const cyan    = T.cyan
  const red     = T.red
  const border  = T.border
  const muted   = T.muted
  const sub     = T.sub
  const emerald = T.emerald
  const card    = '#0d1117'
  const sidebar = T.sidebar

  return (
    <div style={{
      background: T.card, border: `1px solid ${T.border}`,
      borderRadius: 12, padding: '28px 24px 20px', marginBottom: 28,
    }}>
      <svg viewBox={`0 0 ${W} ${H}`} style={{ width: '100%', display: 'block' }}>
        <defs>
          {/* cyan arrowhead */}
          <marker id="sc-arr-c" markerWidth="8" markerHeight="6" refX="7" refY="3" orient="auto">
            <polygon points="0 0, 8 3, 0 6" fill={cyan} />
          </marker>
          {/* grey arrowhead */}
          <marker id="sc-arr-g" markerWidth="8" markerHeight="6" refX="7" refY="3" orient="auto">
            <polygon points="0 0, 8 3, 0 6" fill={muted} />
          </marker>
          {/* red arrowhead */}
          <marker id="sc-arr-r" markerWidth="8" markerHeight="6" refX="7" refY="3" orient="auto">
            <polygon points="0 0, 8 3, 0 6" fill={red} />
          </marker>
          {/* logo glow */}
          <filter id="sc-glow" x="-60%" y="-60%" width="220%" height="220%">
            <feGaussianBlur in="SourceGraphic" stdDeviation="5" result="blur"/>
            <feMerge><feMergeNode in="blur"/><feMergeNode in="SourceGraphic"/></feMerge>
          </filter>
          {/* subtle card gradient */}
          <linearGradient id="sc-left-grad" x1="0" y1="0" x2="0" y2="1">
            <stop offset="0%" stopColor="#111827" />
            <stop offset="100%" stopColor={card} />
          </linearGradient>
          <linearGradient id="sc-right-grad" x1="0" y1="0" x2="0" y2="1">
            <stop offset="0%" stopColor="#071a12" />
            <stop offset="100%" stopColor={card} />
          </linearGradient>
        </defs>

        {/* ── LEFT BOX — raw tool output ───────────────────── */}
        <rect x="8" y="52" width="185" height="122" rx="10"
          fill="url(#sc-left-grad)" stroke={border} strokeWidth="1.5" />
        {/* top label band */}
        <rect x="8" y="52" width="185" height="32" rx="10"
          fill={sidebar} stroke="none" />
        <rect x="8" y="68" width="185" height="16" fill={sidebar} stroke="none" />
        <text x="100" y="73" textAnchor="middle"
          fill={sub} fontSize="10.5" fontWeight="700" fontFamily="Inter,sans-serif" letterSpacing="0.06em">
          RAW TOOL OUTPUT
        </text>
        <text x="100" y="98" textAnchor="middle" fill={muted} fontSize="10" fontFamily="Inter,sans-serif">cargo build · pytest</text>
        <text x="100" y="114" textAnchor="middle" fill={muted} fontSize="10" fontFamily="Inter,sans-serif">npm install · docker build</text>
        <text x="100" y="130" textAnchor="middle" fill={muted} fontSize="10" fontFamily="Inter,sans-serif">git diff · terraform plan</text>
        <text x="100" y="148" textAnchor="middle" fill={muted} fontSize="10" fontFamily="Inter,sans-serif">go test · …</text>
        {/* "noisy" badge */}
        <rect x="56" y="157" width="88" height="13" rx="4"
          fill="rgba(239,68,68,0.1)" stroke="rgba(239,68,68,0.25)" strokeWidth="1" />
        <text x="100" y="167" textAnchor="middle" fill={red} fontSize="9" fontFamily="Inter,sans-serif">thousands of tokens</text>

        {/* ── ARROW: left → logo (dashed, grey — interception) */}
        <path d="M 193 113 L 298 113"
          stroke={muted} strokeWidth="1.5" strokeDasharray="5 3"
          fill="none" markerEnd="url(#sc-arr-g)" />
        <text x="246" y="106" textAnchor="middle"
          fill={muted} fontSize="9.5" fontFamily="Inter,sans-serif">intercepted</text>

        {/* ── CENTER — logo hub ──────────────────────────────── */}
        {/* outer glow ring */}
        <circle cx="390" cy="113" r="64"
          fill="rgba(34,211,238,0.04)"
          stroke="rgba(34,211,238,0.12)"
          strokeWidth="1"
          filter="url(#sc-glow)" />
        {/* main circle */}
        <circle cx="390" cy="113" r="50"
          fill={sidebar}
          stroke={cyan} strokeWidth="1.5" />
        {/* logo image */}
        <image href={`${import.meta.env.BASE_URL}logo.png`} x="374" y="97" width="32" height="32" />
        {/* "on-device" label inside circle */}
        <text x="390" y="148" textAnchor="middle"
          fill={muted} fontSize="9" fontFamily="Inter,sans-serif" letterSpacing="0.04em">on-device</text>

        {/* ── ARROW: logo → right (solid cyan — filtered) ──── */}
        <path d="M 441 113 L 552 113"
          stroke={cyan} strokeWidth="2"
          fill="none" markerEnd="url(#sc-arr-c)" />
        <text x="497" y="106" textAnchor="middle"
          fill={cyan} fontSize="9.5" fontFamily="Inter,sans-serif" opacity="0.85">filtered</text>

        {/* ── RIGHT BOX — agent context ─────────────────────── */}
        <rect x="557" y="52" width="215" height="122" rx="10"
          fill="url(#sc-right-grad)" stroke={border} strokeWidth="1.5" />
        {/* top label band */}
        <rect x="557" y="52" width="215" height="32" rx="10"
          fill={sidebar} stroke="none" />
        <rect x="557" y="68" width="215" height="16" fill={sidebar} stroke="none" />
        <text x="665" y="73" textAnchor="middle"
          fill={sub} fontSize="10.5" fontWeight="700" fontFamily="Inter,sans-serif" letterSpacing="0.06em">
          AGENT CONTEXT
        </text>
        <text x="665" y="97" textAnchor="middle" fill={emerald} fontSize="10" fontFamily="Inter,sans-serif">✓  errors &amp; warnings kept</text>
        <text x="665" y="113" textAnchor="middle" fill={emerald} fontSize="10" fontFamily="Inter,sans-serif">✓  test failures kept</text>
        <text x="665" y="129" textAnchor="middle" fill={muted} fontSize="10" fontFamily="Inter,sans-serif">noise removed</text>
        <text x="665" y="145" textAnchor="middle" fill={muted} fontSize="10" fontFamily="Inter,sans-serif">60–99% fewer tokens</text>
        {/* "clean" badge */}
        <rect x="614" y="157" width="100" height="13" rx="4"
          fill="rgba(52,211,153,0.08)" stroke="rgba(52,211,153,0.25)" strokeWidth="1" />
        <text x="665" y="167" textAnchor="middle" fill={emerald} fontSize="9" fontFamily="Inter,sans-serif">nothing useful dropped</text>

        {/* ── ARROW: logo → bottom (dashed red — blocked) ───── */}
        <path d="M 390 163 L 390 208"
          stroke={red} strokeWidth="1.5" strokeDasharray="4 3"
          fill="none" markerEnd="url(#sc-arr-r)" />

        {/* ── BOTTOM BOX — blocked internet ────────────────── */}
        <rect x="286" y="212" width="208" height="34" rx="8"
          fill="rgba(239,68,68,0.07)"
          stroke="rgba(239,68,68,0.28)" strokeWidth="1" />
        <text x="390" y="230" textAnchor="middle"
          fill={red} fontSize="10" fontFamily="Inter,sans-serif" fontWeight="600">
          ✗  internet / external APIs
        </text>
        <text x="390" y="242" textAnchor="middle"
          fill={muted} fontSize="9" fontFamily="Inter,sans-serif">
          nothing leaves your machine
        </text>
      </svg>
    </div>
  )
}

function SectionSecurity() {
  const guarantees = [
    {
      icon: '🔒',
      title: 'Zero network calls',
      color: T.cyan,
      body: 'PandaFilter never opens a socket. The filtering pipeline, BERT model, and all summarization logic run entirely in the local process. No output is ever transmitted anywhere.',
    },
    {
      icon: '🧠',
      title: 'BERT runs on your machine',
      color: T.violet,
      body: 'The all-MiniLM-L6-v2 model is downloaded once from HuggingFace, then cached at ~/.local/share/ccr/fastembed. After that, it never phones home. Inference is synchronous, single-process, no daemon.',
    },
    {
      icon: '🗂',
      title: 'Session data stays local',
      color: T.emerald,
      body: 'Session state (token counters, result cache, read history) is written to ~/.local/share/panda/sessions/<id>.json. It never leaves disk. Run panda gain to read it; rm -rf ~/.local/share/panda to wipe it.',
    },
    {
      icon: '🔐',
      title: 'Hook integrity enforcement',
      color: T.amber,
      body: 'panda init writes SHA-256 checksums for every installed hook script (chmod 0o444). PandaFilter verifies them at every invocation and exits 1 with a warning if tampered. panda verify checks all agents at once.',
    },
    {
      icon: '📖',
      title: 'What we see: nothing',
      color: T.cyan,
      body: 'PandaFilter is a local filter — it reads the output of your commands, processes it in-memory, and writes the result back. Neither the raw output nor the filtered result is logged, stored beyond the session cache, or accessible by anyone other than you.',
    },
    {
      icon: '🔍',
      title: 'Open source — verify for yourself',
      color: T.indigo,
      body: 'The entire codebase is MIT-licensed and public on GitHub. Every claim on this page is verifiable by reading the source. Search for "reqwest", "ureq", or any HTTP client crate — you\'ll find none.',
    },
  ]

  return (
    <>
      <H2 id="security">Privacy & security</H2>
      <P>
        PandaFilter sits between your AI agent and your shell. Here is exactly what it does — and
        doesn't do — with what it sees.
      </P>

      <SecurityDiagram />

      <P>
        Every command your agent runs passes through PandaFilter locally. The raw output never
        leaves your machine — it goes in, gets filtered in memory, and the cleaned result goes
        back to the agent. That is the entire data path.
      </P>

      <div style={{
        display: 'grid', gridTemplateColumns: '1fr 1fr', gap: 12, marginBottom: 28,
      }}>
        {guarantees.map(({ icon, title, color, body }) => (
          <div key={title} style={{
            background: T.card, border: `1px solid ${T.border}`, borderRadius: 10,
            padding: '18px 20px',
          }}>
            <div style={{ display: 'flex', alignItems: 'center', gap: 10, marginBottom: 10 }}>
              <span style={{ fontSize: 17 }}>{icon}</span>
              <span style={{ fontSize: 13.5, fontWeight: 700, color }}>{title}</span>
            </div>
            <p style={{ fontSize: 13, color: T.sub, lineHeight: 1.65, margin: 0 }}>{body}</p>
          </div>
        ))}
      </div>

      <Callout type="note">
        Want to double-check? The network surface of the binary is zero at filter time.
        You can verify with{' '}<Code>lsof -c panda -i</Code> while a session is running — no open sockets.
        All BERT inference uses the{' '}<Code>fastembed</Code> Rust crate, which loads the model from disk
        and runs it in-process.
      </Callout>
    </>
  )
}

function SectionFAQ() {
  const faqs = [
    {
      q: 'Does PandaFilter change what the agent can see?',
      a: 'It removes noise — build progress, passing test lines, module download logs. Errors, file paths, and results are always kept.',
    },
    {
      q: 'What if I don\'t want a specific command filtered?',
      a: 'Add a rule to .panda/filters.toml to customize or override any handler. You can also use panda proxy <cmd> to run a command raw with no filtering.',
    },
    {
      q: 'What about commands PandaFilter doesn\'t know?',
      a: 'Output passes through unchanged. PandaFilter never silently drops output from unknown commands.',
    },
    {
      q: 'Does PandaFilter send any data outside my machine?',
      a: 'No. All processing is fully local. BERT runs on-device. No telemetry, no network calls during filtering.',
    },
    {
      q: 'What is Context Focusing?',
      a: 'An opt-in feature that tells the agent which files are relevant for the current prompt, preventing it from reading unrelated files. Enable with panda focus --enable after running panda doctor.',
    },
    {
      q: 'How do I verify it\'s working?',
      a: 'Run panda gain after a session. To see exactly what the agent received from a specific command: panda run git log --oneline -20.',
    },
  ]
  return (
    <>
      <H2 id="faq">FAQ</H2>
      <div style={{ display: 'flex', flexDirection: 'column', gap: 2 }}>
        {faqs.map(({ q, a }) => (
          <div key={q} style={{
            background: T.card, border: `1px solid ${T.border}`, borderRadius: 10,
            padding: '18px 22px',
          }}>
            <p style={{ fontSize: 14, fontWeight: 600, color: T.text, marginBottom: 8 }}>{q}</p>
            <p style={{ fontSize: 13.5, color: T.sub, lineHeight: 1.7, margin: 0 }}>{a}</p>
          </div>
        ))}
      </div>
    </>
  )
}

function SectionContributing() {
  return (
    <>
      <H2 id="contributing">Contributing</H2>
      <P>
        Open an issue or PR on{' '}
        <a href="https://github.com/AssafWoo/PandaFilter" style={{ color: T.link }}>GitHub</a>.
        To add a handler: implement the <Code>Handler</Code> trait and register it in{' '}
        <Code>ccr/src/handlers/mod.rs</Code> — see <Code>git.rs</Code> as a template.
      </P>
      <P>
        The codebase is organized into four crates:
      </P>
      <CodeBlock lang="text">{`
ccr/        CLI binary (panda) — handlers, hooks, session state, commands
ccr-core/   Core library (no I/O) — pipeline, BERT summarizer, config, analytics
ccr-sdk/    Conversation compression — tiered compressor, deduplicator, Ollama
ccr-eval/   Evaluation suite — fixtures against Claude API
config/     Embedded default filter patterns
      `}</CodeBlock>
      <Callout type="note">
        MIT licensed. See <a href="https://github.com/AssafWoo/PandaFilter/blob/main/LICENSE" style={{ color: T.link }}>LICENSE</a> for details.
        Built by <a href="https://x.com/AssafPetronio" style={{ color: T.link }}>Assaf Petronio</a>.
      </Callout>
    </>
  )
}

// ─── Search index ──────────────────────────────────────────────────────────────
const SEARCH_INDEX = [
  { id: 'overview',       section: 'Getting Started', title: 'Overview',           keywords: 'pandafilter what is introduction token bill noise context window agent' },
  { id: 'install',        section: 'Getting Started', title: 'Install',            keywords: 'brew homebrew curl linux install setup macos' },
  { id: 'quick-start',    section: 'Getting Started', title: 'Quick start',        keywords: 'panda init cursor gemini cline copilot wire agent doctor' },
  { id: 'agents',          section: 'How It Works',    title: 'Supported agents',   keywords: 'agents claude cursor gemini cline copilot vscode init hook pretooluse posttooluse usersubmit supported' },
  { id: 'pipeline',       section: 'How It Works',    title: 'Filtering pipeline', keywords: 'pipeline ansi whitespace regex ndjson summarize bert cap stage steps 200k 50k' },
  { id: 'bert',           section: 'How It Works',    title: 'BERT engine',        keywords: 'bert model embedding miniLM 384 dimensions anomaly cosine similarity noise useful entropy adaptive budget weights' },
  { id: 'handlers',       section: 'How It Works',    title: 'Handlers',           keywords: 'handlers cargo git pytest jest npm docker kubectl terraform eslint tsc 59 routing' },
  { id: 'focus',          section: 'How It Works',    title: 'Context focusing',   keywords: 'focus index file retrieval cochange read history boost role entry point opt-in panda focus enable' },
  { id: 'token-savings',  section: 'Benchmarks',      title: 'Token savings',      keywords: 'tokens savings 82% pip docker swift cargo before after benchmark panda gain handler reduction' },
  { id: 'file-retrieval', section: 'Benchmarks',      title: 'File retrieval',     keywords: 'retrieval hit@5 mrr accuracy 85.6% hybrid lexical semantic ranking repos benchmark 90 tasks' },
  { id: 'commands',       section: 'Reference',       title: 'Commands',           keywords: 'panda gain focus index doctor verify discover run proxy expand noise compress uninstall commands' },
  { id: 'config',         section: 'Reference',       title: 'Configuration',      keywords: 'config toml panda.toml filters user-defined patterns remove collapse replace truncate on_empty' },
  { id: 'security',       section: 'More',            title: 'Privacy & security', keywords: 'privacy security local no network no telemetry no api calls data open source sha256 hook integrity on-device bert' },
  { id: 'faq',            section: 'More',            title: 'FAQ',                keywords: 'faq questions answers data local does panda change verify working filters unknown' },
  { id: 'contributing',   section: 'More',            title: 'Contributing',       keywords: 'contributing github handler trait mod.rs crates ccr ccr-core ccr-sdk ccr-eval license mit' },
]

// ─── App shell ────────────────────────────────────────────────────────────────
export default function App() {
  const [activeSection, setActiveSection] = useState('overview')
  const [query, setQuery] = useState('')
  const [searchFocused, setSearchFocused] = useState(false)
  const [selectedIdx, setSelectedIdx] = useState(0)
  const searchRef = useRef<HTMLInputElement>(null)
  const contentRef = useRef<HTMLDivElement>(null)

  const results = query.trim().length > 0
    ? SEARCH_INDEX.filter(item => {
        const q = query.toLowerCase()
        return item.title.toLowerCase().includes(q) ||
               item.keywords.toLowerCase().includes(q) ||
               item.section.toLowerCase().includes(q)
      })
    : []

  const showDropdown = searchFocused && results.length > 0

  function goTo(id: string) {
    setQuery('')
    setSearchFocused(false)
    document.getElementById(id)?.scrollIntoView({ behavior: 'smooth' })
  }

  useEffect(() => { setSelectedIdx(0) }, [query])

  useEffect(() => {
    const observer = new IntersectionObserver(
      entries => {
        entries.forEach(e => { if (e.isIntersecting) setActiveSection(e.target.id) })
      },
      { rootMargin: '-20% 0% -60% 0%' }
    )
    TOC_ITEMS.forEach(({ id }) => {
      const el = document.getElementById(id)
      if (el) observer.observe(el)
    })
    return () => observer.disconnect()
  }, [])

  return (
    <div style={{ background: T.bg, color: T.text, minHeight: '100vh', fontFamily: 'Inter, system-ui, sans-serif' }}>
      {/* Top nav */}
      <nav style={{
        position: 'fixed', top: 0, left: 0, right: 0, zIndex: 100,
        height: 56, display: 'flex', alignItems: 'center', justifyContent: 'space-between',
        padding: '0 28px', background: T.sidebar, borderBottom: `1px solid ${T.border}`,
      }}>
        <div style={{ display: 'flex', alignItems: 'center', gap: 10 }}>
          <img src={`${import.meta.env.BASE_URL}logo.png`} alt="PandaFilter" style={{ width: 32, height: 32, objectFit: 'contain' }} />
          <span style={{ fontWeight: 700, fontSize: 15, color: T.text }}>PandaFilter</span>
          <span style={{
            fontSize: 11, padding: '2px 8px', borderRadius: 999,
            background: 'rgba(34,211,238,0.1)', color: T.cyan, border: `1px solid rgba(34,211,238,0.25)`,
            marginLeft: 4,
          }}>docs</span>
        </div>
        <div style={{ display: 'flex', gap: 24, alignItems: 'center' }}>
          {[
            { label: 'GitHub',  href: 'https://github.com/AssafWoo/PandaFilter' },
            { label: 'Discord', href: 'https://discord.com/invite/FFQC3bxYQ'    },
          ].map(({ label, href }) => (
            <a key={label} href={href} style={{ fontSize: 13.5, color: T.sub, textDecoration: 'none' }}
              onMouseEnter={e => (e.currentTarget.style.color = T.text)}
              onMouseLeave={e => (e.currentTarget.style.color = T.sub)}>
              {label}
            </a>
          ))}
        </div>
      </nav>

      {/* Body */}
      <div style={{ display: 'flex', paddingTop: 56, minHeight: '100vh' }}>

        {/* Left sidebar */}
        <aside style={{
          width: 240, flexShrink: 0, position: 'fixed', top: 56, bottom: 0,
          background: T.sidebar, borderRight: `1px solid ${T.border}`,
          overflowY: 'auto', display: 'flex', flexDirection: 'column',
        }}>
          {/* Search */}
          <div style={{ padding: '16px 16px 12px', position: 'relative', flexShrink: 0 }}>
            <div style={{
              display: 'flex', alignItems: 'center', gap: 8,
              background: T.code, border: `1px solid ${searchFocused ? T.cyan : T.border}`,
              borderRadius: 8, padding: '7px 12px',
              transition: 'border-color 0.15s',
            }}>
              <svg width="14" height="14" viewBox="0 0 16 16" fill="none" style={{ flexShrink: 0 }}>
                <circle cx="6.5" cy="6.5" r="5" stroke={T.muted} strokeWidth="1.5"/>
                <path d="M10.5 10.5L14 14" stroke={T.muted} strokeWidth="1.5" strokeLinecap="round"/>
              </svg>
              <input
                ref={searchRef}
                value={query}
                onChange={e => setQuery(e.target.value)}
                onFocus={() => setSearchFocused(true)}
                onBlur={() => setTimeout(() => setSearchFocused(false), 150)}
                onKeyDown={e => {
                  if (e.key === 'ArrowDown') { e.preventDefault(); setSelectedIdx(i => Math.min(i + 1, results.length - 1)) }
                  if (e.key === 'ArrowUp')   { e.preventDefault(); setSelectedIdx(i => Math.max(i - 1, 0)) }
                  if (e.key === 'Enter' && results[selectedIdx]) goTo(results[selectedIdx].id)
                  if (e.key === 'Escape') { setQuery(''); setSearchFocused(false) }
                }}
                placeholder="Search docs..."
                style={{
                  background: 'transparent', border: 'none', outline: 'none',
                  fontSize: 13, color: T.text, width: '100%',
                  '::placeholder': { color: T.muted },
                } as React.CSSProperties}
              />
              {query && (
                <button onClick={() => setQuery('')} style={{
                  background: 'none', border: 'none', cursor: 'pointer',
                  color: T.muted, fontSize: 16, lineHeight: 1, padding: 0, flexShrink: 0,
                }}>×</button>
              )}
            </div>

            {/* Dropdown results */}
            {showDropdown && (
              <div style={{
                position: 'absolute', top: '100%', left: 16, right: 16, zIndex: 200,
                background: T.card, border: `1px solid ${T.border}`, borderRadius: 8,
                boxShadow: '0 8px 24px rgba(0,0,0,0.4)', overflow: 'hidden',
                marginTop: -4,
              }}>
                {results.map((item, i) => (
                  <div
                    key={item.id}
                    onMouseDown={() => goTo(item.id)}
                    onMouseEnter={() => setSelectedIdx(i)}
                    style={{
                      padding: '10px 14px', cursor: 'pointer',
                      background: i === selectedIdx ? 'rgba(34,211,238,0.08)' : 'transparent',
                      borderBottom: i < results.length - 1 ? `1px solid ${T.borderSoft}` : 'none',
                    }}
                  >
                    <div style={{ fontSize: 13, fontWeight: 500, color: i === selectedIdx ? T.cyan : T.text }}>
                      {item.title}
                    </div>
                    <div style={{ fontSize: 11, color: T.muted, marginTop: 2 }}>{item.section}</div>
                  </div>
                ))}
              </div>
            )}

            {/* No results */}
            {searchFocused && query.trim().length > 0 && results.length === 0 && (
              <div style={{
                position: 'absolute', top: '100%', left: 16, right: 16, zIndex: 200,
                background: T.card, border: `1px solid ${T.border}`, borderRadius: 8,
                padding: '12px 14px', marginTop: -4,
                boxShadow: '0 8px 24px rgba(0,0,0,0.4)',
              }}>
                <span style={{ fontSize: 13, color: T.muted }}>No results for "{query}"</span>
              </div>
            )}
          </div>

          {/* Nav groups */}
          <div style={{ flex: 1, overflowY: 'auto', padding: '4px 0 28px' }}>
            {NAV.map(({ group, items }) => (
              <div key={group} style={{ marginBottom: 24 }}>
                <div style={{
                  padding: '0 20px 8px', fontSize: 11, fontWeight: 700, color: T.muted,
                  letterSpacing: '0.07em', textTransform: 'uppercase',
                }}>
                  {group}
                </div>
                {items.map(({ id, label }) => {
                  const isActive = activeSection === id
                  return (
                    <a key={id} href={`#${id}`}
                      onClick={e => { e.preventDefault(); goTo(id) }}
                      style={{
                        display: 'block', padding: '7px 20px', fontSize: 13.5,
                        color: isActive ? T.cyan : T.sub,
                        background: isActive ? 'rgba(34,211,238,0.07)' : 'transparent',
                        borderLeft: isActive ? `2px solid ${T.cyan}` : '2px solid transparent',
                        textDecoration: 'none', cursor: 'pointer',
                        transition: 'all 0.1s',
                      }}>
                      {label}
                    </a>
                  )
                })}
              </div>
            ))}
          </div>
        </aside>

        {/* Main content */}
        <main ref={contentRef} style={{
          flex: 1, marginLeft: 240, marginRight: 200,
          padding: '48px 56px 96px',
          minWidth: 0,
        }}>
          <div id="overview">
            <SectionOverview />
          </div>
          <SectionInstall />
          <SectionAgents />
          <SectionPipeline />
          <SectionBert />
          <SectionHandlers />
          <SectionFocus />
          <SectionTokenSavings />
          <SectionRetrieval />
          <SectionCommands />
          <SectionConfig />
          <SectionSecurity />
          <SectionFAQ />
          <SectionContributing />

          {/* Footer */}
          <div style={{
            marginTop: 64, paddingTop: 32, borderTop: `1px solid ${T.border}`,
            fontSize: 12, color: T.muted, display: 'flex', justifyContent: 'space-between',
          }}>
            <span>PandaFilter · MIT License</span>
            <a href="https://github.com/AssafWoo/PandaFilter" style={{ color: T.muted, textDecoration: 'none' }}>
              Edit on GitHub
            </a>
          </div>
        </main>

        {/* Right TOC */}
        <aside style={{
          width: 200, flexShrink: 0, position: 'fixed', top: 56, right: 0, bottom: 0,
          padding: '28px 20px', overflowY: 'auto',
          borderLeft: `1px solid ${T.border}`,
        }}>
          <div style={{
            fontSize: 11, fontWeight: 700, color: T.muted,
            letterSpacing: '0.07em', textTransform: 'uppercase', marginBottom: 14,
          }}>
            On this page
          </div>
          {TOC_ITEMS.map(({ id, label }) => {
            const isActive = activeSection === id
            return (
              <a key={id} href={`#${id}`}
                onClick={e => { e.preventDefault(); document.getElementById(id)?.scrollIntoView({ behavior: 'smooth' }) }}
                style={{
                  display: 'block', padding: '5px 0', fontSize: 12.5,
                  color: isActive ? T.cyan : T.sub,
                  textDecoration: 'none', cursor: 'pointer',
                  transition: 'color 0.1s',
                }}>
                {label}
              </a>
            )
          })}
        </aside>
      </div>
    </div>
  )
}
