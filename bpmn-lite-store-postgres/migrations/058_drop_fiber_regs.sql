-- The fibers.regs column is vestigial: v1's register file was deleted with
-- the v2 ISA cutover (fibres carry pc + operand stack + control stack; no
-- registers exist in the machine model). The store has written a constant
-- empty array since then and nothing ever reads it back. Greenfield rule:
-- destructive down is acceptable; there is no data to preserve (every row
-- holds '[]').
ALTER TABLE fibers DROP COLUMN regs;
