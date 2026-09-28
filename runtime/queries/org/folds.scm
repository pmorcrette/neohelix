; Outline folding: a section spans its headline, plan, drawers, body and
; every nested subsection.
(section) @fold

; Block-level containers that carry their own begin/end delimiters.
[
  (block)
  (dynamic_block)
  (drawer)
  (property_drawer)
  (latex_env)
  (list)
  (table)
] @fold
