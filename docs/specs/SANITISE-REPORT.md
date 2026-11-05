# Sanitise report

Review of the behaviour specs before publication. Each entry names a location and the kind of content that was removed or
rewritten; the removed text is not quoted. Numbers, formulas and behaviours were not changed. Every `SPEC.md` also got a
"Sanitised for publication" note under its title. In every `LOGS.md`/`README.md`, absolute scratch paths were replaced by
relative descriptions and mentions of the spec team's private folder were dropped (the one remaining mention is in
`README.md` of this directory).

## dl

SPEC.md (4 passages)
- Preface, paragraph 1: reference to private reading of the original and to the `logs/` data files, reworded.
- Section 4, opening paragraph: pointer to another spec file replaced by an inline statement of the header offsets.
- Section 4, `steps` item: prose description of the two instruction forms of the volume-store search replaced by the two 4-byte window constants plus a short behavioural description.
- Section 5.1, first paragraph: description of the module init's straight-line/loop structure replaced by a statement of what it costs and does.

LOGS.md (rewritten): scratch directory paths, reference binary path, module path, port-log paths, a shell command block with scratch paths, and a note about the private trace folder.
README.md (rewritten): the paragraph naming the private folder and what it holds.

## dl2

SPEC.md (3 passages)
- Header, "Validation data" line: note that the logs are not shipped.
- Section 4, first paragraph: assembler-style description of the two volume-store forms replaced by the window constants and a behavioural description.
- Section 6, X paragraph: assembler-level wording of an address-load difference between two builds replaced by the cycle numbers only.

LOGS.md (rewritten): scratch root path, script directory path, mention of private traces and Eagleplayer dumps.
README.md (rewritten): "decision for the user" wording only.

## dl3

SPEC.md (18 passages)
- Header, cycle-cost line: mnemonic names for the instruction cost classes replaced by plain class names.
- Section 0, paragraph 1: textual instruction (call + no-op padding) used to describe the patch replaced by a prose description with the opcode constant.
- Section 0, paragraph 2: instruction name in the design advice.
- Section 0, cycle accounting sentence: mnemonic names.
- Section 1, paragraph 1: mnemonic names in the site replacement description.
- Section 1, module-shape table, "Meaning" column: eleven rows of assembler text (operands, registers, addressing modes) rewritten as behaviour; the hex search keys were kept.
- Section 1, module-shape table, R9 row: assembler text.
- Section 1, R9 paragraph: assembler text.
- Section 1, paragraph "Before patching": branch mnemonic, and the explanation of the displacement edit, rewritten.
- Section 2, paragraph 3: absolute address of a pseudo-channel in one module, replaced by a description.
- Section 2, paragraph 4: assembler text describing an extra store of the stubs.
- Section 2, paragraph 4: assembler text describing the post-increment read.
- Section 3, time-of-store paragraph and section 3.1 intro and R12 line: call/return mnemonic wording replaced by plain words (4 small edits, counted as one).

LOGS.md (rewritten): root, reference binary, reference log and module scratch paths, "private work copy" remark.
README.md (rewritten): two first-person remarks reworded; one sentence about unreproducible table content reworded.

## jesperolsen

SPEC.md: no passage needed removal (already behaviour-only); the title note was added.
LOGS.md (rewritten): scratch logs/module/ID-list paths, the compare-script path, and the module directory path.
README.md: unchanged apart from line wrapping.

## musicline

SPEC.md (2 passages)
- Section 6, first paragraph: internal scratch name of the isolation setup.
- Section 6, last sentence: note that `logs/` is not shipped.

LOGS.md (rewritten): scratch binary location, mentions of a private trace tool and a trace build, corpus path.
README.md: one sentence about the private folder removed.

## davidhanney

SPEC.md: no passage needed removal (already behaviour-only); the title note was added. The period table (section 4.5) and arpeggio table (section 4.6) were kept: they are the format's own data (69 and 14x12 entries).
LOGS.md (rewritten): scratch log paths, the archive location of the corpus, the reference binary location.
README.md (rewritten): reference to private extraction notes with file offsets removed; credit string naming the adaptors of the Eagleplayer removed; "the lead" changed to "the maintainer".

## markcooksey

SPEC.md (2 passages)
- Section 4.1, paragraph 1: assembler mnemonic and operand form of the address-load opcode.
- Section 6.1, paragraph 2: assembler mnemonic and register of the address-load opcode.

LOGS.md (rewritten): scratch folder, module database and archive paths, reference binary path, the remark about private reading notes.
README.md: one reference to a private module-index row reworded.

## robhubbard

SPEC.md (3 passages)
- Section 1.2, detection constants: description of the second constant as a small return plus an address load, replaced by a neutral phrase (constants kept).
- Section 1.3, "Finding them in a module that is not listed" (whole block, 6 bullets): register-by-register recipe of the init code (register names, instruction forms, immediates) rewritten as a prose list of which pointer is loaded where; no value changed.
- Section 8.6, first line: parenthetical describing the code structure of the next-pattern path.

LOGS.md (rewritten): scratch binary path, scratch log directory references, private-module remark.
README.md: one phrase ("from the code's arithmetic") shortened.
