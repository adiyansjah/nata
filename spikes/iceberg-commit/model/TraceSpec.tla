------------------------------ MODULE TraceSpec ------------------------------
(***************************************************************************)
(* TRACE VALIDATION (verification-plan item 24, the code gate). The model  *)
(* of IcebergCommit.tla, restricted to ONE recorded behaviour of the Rust  *)
(* writers. `Trace` is a sequence of abstract events written by            *)
(* model/trace_to_tla.py from a trace of the code (src/trace.rs); at step  *)
(* i the specification can take only the model action that event i names,  *)
(* with the numbers the code observed (attempt, base and expected version, *)
(* the version the catalog was at). The model's own Next is NOT used: every *)
(* step is one of its actions, unchanged, with the event's numbers as      *)
(* extra guards.                                                           *)
(*                                                                         *)
(* The trace is VALID iff TLC can reach i = Len(Trace)+1, i.e. the         *)
(* invariant NotDone is VIOLATED. If TLC instead deadlocks, the event at   *)
(* the i printed in the last state is one the model does not allow there:  *)
(* the code did something the model forbids (or the model is wrong).       *)
(* Everything the trace does not say is chosen by the model: what the      *)
(* writer planned (from the rows, the keys and the table), the metadata    *)
(* the store writer builds, the validation outcome. Two events only compare *)
(* the code with the model, without moving it: `planned` (how many delete  *)
(* targets, compaction inputs and rows the writer planned) and `final` (the *)
(* version and the live rows of the table when every writer is done).      *)
(***************************************************************************)
EXTENDS IcebergCommit

CONSTANT Trace
VARIABLE i

tvars == <<vars, i>>

\* The catalog delivers writer e.w's request of attempt e.att; the catalog was at version e.cur.
\* A duplicated request has keep = TRUE on its first delivery (it stays in the network).
Delivery(e) ==
    \E q \in net :
        /\ q.w = e.w /\ q.k = e.att /\ q.exp = e.exp
        /\ CurVer = e.cur
        /\ IF e.res = "ok"
              THEN /\ TargetVer(q) = e.new
                   /\ DeliverOk(q, e.keep)
              ELSE DeliverConflict(q, e.keep)

\* Re-read after a 409 (e.from = "conflict") or an unknown outcome ("unknown"), then a decision.
Recov(e) ==
    /\ CurVer = e.ver
    /\ att[e.w] = e.att
    /\ pc[e.w] = IF e.from = "conflict" THEN "conflicted" ELSE "unknown"

Step(e) ==
    CASE e.a = "plan"           -> CurVer = e.ver /\ PlanOp(e.w)
      [] e.a = "upload-begin"   -> BeginUpload(e.w)
      [] e.a = "upload-finish"  -> UpFile(e.w) \in inflight /\ FinishUpload(UpFile(e.w))
      [] e.a = "upload-await"   -> AwaitUpload(e.w)
      [] e.a = "build"          -> att[e.w] = e.att /\ base[e.w] = e.base /\ WriteMeta(e.w)
      [] e.a = "send"           -> att[e.w] = e.att /\ exp[e.w] = e.exp /\ Send(e.w)
      [] e.a = "deliver"        -> Delivery(e)
      [] e.a = "drop"           -> \E q \in net : q.w = e.w /\ q.k = e.att /\ Drop(q)
      [] e.a = "receive"        -> att[e.w] = e.att /\ resp[e.w] = e.res /\ Receive(e.w)
      [] e.a = "timeout"        -> att[e.w] = e.att /\ Timeout(e.w)
      [] e.a = "recover-done"   -> Recov(e) /\ RecoverDone(e.w)
      [] e.a = "recover-abort"  -> Recov(e) /\ RecoverAbort(e.w)
      [] e.a = "recover-retry"  -> Recov(e) /\ RecoverRetry(e.w)
      [] e.a = "recover-fail"   -> Recov(e) /\ RecoverFail(e.w)
      [] e.a = "crash"          -> Crash(e.w)
      [] e.a = "outcome"        -> pc[e.w] = e.pc /\ UNCHANGED vars
      [] e.a = "planned"        -> /\ Cardinality(plan[e.w].targets) = e.targets
                                   /\ Cardinality(plan[e.w].inputs) = e.inputs
                                   /\ (OpOf[e.w] = "delete" \/ Cardinality(frows[Data(e.w)]) = e.rows)
                                   /\ UNCHANGED vars
      [] e.a = "final"          -> CurVer = e.ver /\ LiveRows(CurMeta.cur) = e.rows /\ UNCHANGED vars
      [] OTHER                  -> FALSE

TInit == Init /\ i = 1

TNext == /\ i <= Len(Trace)
         /\ Step(Trace[i])
         /\ i' = i + 1

TraceSpec == TInit /\ [][TNext]_tvars

\* Violated exactly when the whole trace has been replayed.
NotDone == i <= Len(Trace)
=============================================================================
