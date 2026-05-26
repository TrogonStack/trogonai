# Standards Crosswalks

Crosswalks from external standards / frameworks to TrogonStack governance controls. Each row points to:

- the standard's clause / threat / criterion,
- the TrogonStack control that satisfies it,
- the owning crate or document,
- gap status (per `05-gap-analysis.md` IDs where applicable).

Crosswalks:

- [owasp-agentic-top-10.md](./owasp-agentic-top-10.md) — OWASP GenAI / Agentic Security Initiative threats (AGT calls these ASI-01..10).
- [nist-ai-rmf.md](./nist-ai-rmf.md) — NIST AI RMF 1.0 core functions and GenAI Profile (AI 600-1).
- [eu-ai-act.md](./eu-ai-act.md) — EU AI Act high-risk system + GPAI obligations.
- [soc2.md](./soc2.md) — SOC 2 Trust Services Criteria.
- [iso-42001.md](./iso-42001.md) — ISO/IEC 42001 AI management system clauses + Annex A.

## Conventions

- **Status**: `met` (control implemented), `partial` (scaffolded / planned), `gap` (no implementation).
- **Owner**: crate or doc that holds the control.
- **Gap IDs**: reference `05-gap-analysis.md`; close the gap and the row becomes `met`.

These crosswalks are *not* a certification claim. They are an internal traceability map so that, when an auditor asks "where do you implement X", we can answer with a subject name, a crate, and a signed artifact.

## Reading Order If Preparing For Certification

1. **SOC 2** first — most common enterprise gate; criteria are operational and the gaps are concrete.
2. **ISO 42001** — AI-specific management system; complements SOC 2.
3. **EU AI Act** — required if shipping to EU customers; obligations are jurisdictional.
4. **NIST AI RMF** — voluntary US framework; useful internal vocabulary.
5. **OWASP Agentic** — threat-level mapping; useful for product security narrative.
