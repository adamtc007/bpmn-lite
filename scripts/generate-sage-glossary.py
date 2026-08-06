#!/usr/bin/env python3
"""Generate the user-facing Sage Designer glossary FROM the admitted
semantic pack (docs are a projection of the pack, never hand-drifted).
Regenerate after any pack change: python3 scripts/generate-sage-glossary.py"""
import yaml, sys
from pathlib import Path

root = Path(__file__).resolve().parent.parent
pack = yaml.safe_load(open(root / 'utterance-engine/config/bpmn-semantic-pack.yaml'))
caps = pack.get('capabilities') or pack.get('candidates') or []

SUPPORT = {
    'supported': 'ready — applies your change directly',
    'needs_workbook': 'ready — will open a short form to collect details first',
    'not_representable': 'recognised but not yet executable — Sage will acknowledge and record it',
}

def section(c):
    ext = c.get('extensions') or {}
    support = SUPPORT.get(ext.get('bpmn.binder_support', ''), '')
    lines = [f"### {c.get('title')} — `{c.get('id')}`", ""]
    lines.append(f"**What it does:** {c.get('intent_summary')}.")
    if c.get('effect'): lines.append(f"**Effect on your workflow:** {c['effect']}.")
    if c.get('applicability'): lines.append(f"**When you can use it:** {c['applicability']}.")
    ph = [f'"{p["text"]}"' for p in (c.get('phrases') or [])]
    ex = [f'"{x}"' for x in (c.get('positive_examples') or [])]
    if ph: lines.append(f"**Say it like:** {', '.join(ph)}.")
    if ex: lines.append(f"**Example:** {', '.join(ex[:2])}.")
    args = [a for a in (c.get('arguments') or []) if a.get('clarification_prompt')]
    if args:
        lines.append("**Sage will ask you:**")
        for a in args:
            req = 'required' if a.get('required') else 'optional'
            lines.append(f"- {a['clarification_prompt']} ({a.get('name')}, {req})")
    nc = c.get('negative_contrasts') or []
    if nc:
        lines.append("**Not to be confused with:**")
        for n in nc:
            lines.append(f"- `{n['candidate_id']}` — {n['distinction']}")
    if support: lines.append(f"**Status:** {support}.")
    lines.append("")
    return '\n'.join(lines)

ops = sorted((c for c in caps if str(c.get('id','')).startswith('op.')), key=lambda c: c['id'])
prods = sorted((c for c in caps if str(c.get('id','')).startswith('prod.')), key=lambda c: c['id'])

doc = ["# Sage Designer glossary — how to instruct the workflow designer",
"",
"*Generated from the admitted semantic pack (`bpmn-semantic-pack.yaml`) by",
"`scripts/generate-sage-glossary.py` — regenerate after any pack change; never edit by hand.*",
"",
"## How Sage listens",
"",
"- **You speak; the graph decides what is possible.** At any moment Sage only",
"  considers actions that are *legal at your selected node* — you cannot be",
"  offered an edit that would break the workflow's structure.",
"- **One action per utterance.** \"Add a review step after triage\" lands;",
"  \"add a review step and also time it out and loop legal in\" forces Sage to",
"  ask you to split it.",
"- **Name your anchor.** Say *where* — \"after the screening step\", \"on this",
"  gateway\" — or select the node first and say \"here\" / \"this one\".",
"- **Say the distinguishing consequence for routing.** Exactly one route wins →",
"  an exclusive branch. Every branch always runs → parallel. The branches whose",
"  conditions hold run (one, some, or all) → inclusive.",
"- **If nothing fits, Sage abstains** rather than guessing — asking a question",
"  or saying it can't represent the request is correct behaviour, not failure.",
"- **Everything is reversible until you ratify.** Proposals stage first; nothing",
"  changes your workflow without your explicit accept.",
"",
"## Building-block operations",
""]
doc += [section(c) for c in ops]
doc += ["## Ready-made patterns (productions)", "",
"These create a whole governed shape in one instruction — a request-and-wait,",
"a reminder-then-escalate cycle, a timeout route — instead of node-by-node assembly.",
""]
doc += [section(c) for c in prods]

out = root / 'docs/sage-designer-glossary.md'
out.write_text('\n'.join(doc))
print(f"wrote {out} ({len(ops)} operations, {len(prods)} productions)")
