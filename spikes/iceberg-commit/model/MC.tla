------------------------------- MODULE MC -----------------------------------
(* Instances for the model checker: the constants that are functions are given here and the
   configuration files pick them with "Name <- Definition". *)
EXTENDS IcebergCommit, TLC

\* Rows 1 (key a) and 2 (key b) are in the initial file; 3 (a) and 4 (b) are appended by writers.
MCKey == (1 :> "a") @@ (2 :> "b") @@ (3 :> "a") @@ (4 :> "b")

\* One writer: an appender.
M1Op  == (1 :> "append")
M1App == (1 :> {3})
M1Del == (1 :> "-")

\* Two appenders (lost update, duplicate append, unknown outcome).
M2Op  == (1 :> "append") @@ (2 :> "append")
M2App == (1 :> {3}) @@ (2 :> {4})
M2Del == (1 :> "-") @@ (2 :> "-")

\* An appender of a row with key a and a delete-where key a (phantom under snapshot isolation).
MadOp  == (1 :> "append") @@ (2 :> "delete")
MadApp == (1 :> {3}) @@ (2 :> {})
MadDel == (1 :> "-") @@ (2 :> "a")

\* A delete-where key a and a compaction (the compaction must not resurrect the deleted row).
MdcOp  == (1 :> "delete") @@ (2 :> "compact")
MdcApp == (1 :> {}) @@ (2 :> {})
MdcDel == (1 :> "a") @@ (2 :> "-")

\* Appender, delete-where key a, compaction.
M3Op  == (1 :> "append") @@ (2 :> "delete") @@ (3 :> "compact")
M3App == (1 :> {3}) @@ (2 :> {}) @@ (3 :> {})
M3Del == (1 :> "-") @@ (2 :> "a") @@ (3 :> "-")

\* Two delete-where key a (both plan the same pair) and a compaction: the second deleter retries and
\* commits a REDUNDANT delete file; a compaction that planned in between must still conflict with it.
MddcOp  == (1 :> "delete") @@ (2 :> "delete") @@ (3 :> "compact")
MddcApp == (1 :> {}) @@ (2 :> {}) @@ (3 :> {})
MddcDel == (1 :> "a") @@ (2 :> "a") @@ (3 :> "-")
=============================================================================
