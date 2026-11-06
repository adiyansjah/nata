------------------------------ MODULE IcebergCommit ------------------------------
(***************************************************************************)
(* THROWAWAY SPIKE (verification-plan item 24). A model of the Apache      *)
(* Iceberg commit protocol for ONE table with several concurrent writers.  *)
(*                                                                         *)
(* What it models                                                          *)
(*   - A catalog that holds the metadata versions of the table: version n  *)
(*     is the table metadata after the n-th commit. The current version is *)
(*     the highest one. Style "REST": the server applies the writer's      *)
(*     AddSnapshot to the CURRENT metadata when the requirement            *)
(*     (assert-ref-snapshot-id, abstracted to "the table is still at the   *)
(*     version I read") holds. Style "STORE": no server; the writer builds *)
(*     the whole next metadata file from its own base and creates it with  *)
(*     put-if-absent (PutMode::Create, S3 If-None-Match, GCS               *)
(*     ifGenerationMatch=0, Azure If-None-Match). Both are one atomic      *)
(*     compare-and-swap on the version number; they differ in WHO builds   *)
(*     the new metadata, which matters when the CAS is removed.            *)
(*   - Writers, one operation each: "append" a row, "delete" all rows of a *)
(*     key (a delete-where, position deletes), "compact" (rewrite all live *)
(*     files into one, applying the deletes). Each writer: reads the       *)
(*     table (plan), uploads its data or delete file, waits until it is    *)
(*     durable, writes a manifest list, sends the commit, waits for the    *)
(*     answer. On 409 (conflict) or on an unknown outcome (timeout) it     *)
(*     re-reads, first looks for its own snapshot (the operation token),   *)
(*     then VALIDATES against the snapshots committed since its base       *)
(*     (Iceberg's rules: replace and delete must find their files still    *)
(*     in the table, a compaction must find no delete FILE committed since *)
(*     its base that names one of its inputs, whether or not that file     *)
(*     deletes rows nobody had deleted yet (Iceberg's rewrite validation,  *)
(*     validateNoNewDeletesForDataFiles), a delete under serializable      *)
(*     isolation must find no new matching rows), then rebuilds on the new *)
(*     base and retries, a bounded number of times.                        *)
(*   - Faults: a writer can crash at ANY step (budget); a request can be   *)
(*     lost or delivered twice (budgets); a response is never "lost" but a *)
(*     writer can time out at any moment while waiting (so the request can *)
(*     still be in flight, already applied, or never delivered:            *)
(*     CommitStateUnknown); orphan files are left behind by crashes and    *)
(*     aborts and collected by a garbage collector that must not delete a  *)
(*     file an active writer or an in-flight request still needs.         *)
(*   - Readers pin a snapshot and must keep seeing exactly its rows.       *)
(*                                                                         *)
(* What it does NOT model (see bench/results/20261104-iceberg-commit.md)   *)
(*   equality deletes, deletion vectors, partitions and partition          *)
(*   evolution, schema changes, branches and tags, snapshot expiry,        *)
(*   multi-table transactions, the REST Idempotency-Key header, clock      *)
(*   time, partial (torn) object writes (PUT is atomic), stale LIST.       *)
(*                                                                         *)
(* Assumption A-GC: the garbage collector's age threshold is larger than   *)
(* the lifetime of any request in flight, which the model states as "no    *)
(* request in the network refers to the file".                             *)
(*                                                                         *)
(* Mutants (model/mutate_model.py) edit one-line definitions marked with   *)
(* the comment "MUTATION POINT"; the script checks each edit applies once. *)
(***************************************************************************)
EXTENDS Integers, FiniteSets, Sequences, TLC

CONSTANTS
    Writers,        \* positive integers; 0 is the initial snapshot
    OpOf,           \* [Writers -> {"append", "delete", "compact"}]
    AppendRows,     \* [Writers -> SUBSET Rows]  rows an appender adds
    DelKey,         \* [Writers -> keys]         predicate of a deleter ("-" for others)
    Rows,           \* set of row ids
    KeyOf,          \* [Rows -> keys]
    InitRows,       \* rows of the initial data file
    Style,          \* "REST" | "STORE"
    Isolation,      \* "serializable" | "snapshot"
    MaxAttempts,    \* commit attempts per writer (1 + retries)
    MaxVer,         \* cap on metadata versions (never binds in the unmutated model)
    MaxPins,        \* readers that pin a snapshot
    CrashBudget, DupBudget, LossBudget

ASSUME Style \in {"REST", "STORE"}
ASSUME Isolation \in {"serializable", "snapshot"}
ASSUME \A w \in Writers : w \in Nat /\ w > 0
ASSUME MaxAttempts \in Nat /\ MaxAttempts > 0 /\ MaxVer \in Nat /\ MaxPins \in Nat
ASSUME CrashBudget \in Nat /\ DupBudget \in Nat /\ LossBudget \in Nat

VARIABLES
    mlog,       \* function: existing version number -> table metadata of that version
    hist,       \* GHOST: sequence of the effects of every successful catalog apply, in order
    durable,    \* files that exist in the object store
    inflight,   \* uploads started and not yet complete (an upload is atomic when it completes)
    frows,      \* GHOST: the rows each data file holds (files are immutable)
    net,        \* commit requests in the network
    resp,       \* the answer a waiting writer has received but not yet read
    pc, att, base, exp, plan, snap,   \* per-writer state
    pinned,     \* readers: [id |-> snapshot id, rows |-> rows they saw]
    crashLeft, dupLeft, lossLeft

ledger == <<mlog, hist>>
filev  == <<durable, inflight, frows>>
netv   == <<net, resp>>
wl     == <<pc, att, base, exp, plan, snap>>
bud    == <<crashLeft, dupLeft, lossLeft>>
vars   == <<ledger, filev, netv, wl, pinned, bud>>

(* ------------------------------------------------------------------------ *)
(* Files and snapshots                                                      *)
(* ------------------------------------------------------------------------ *)
Data(w)   == <<"d", w, 0>>
Del(w)    == <<"x", w, 0>>
Man(w, k) == <<"m", w, k>>
F0 == Data(0)
M0 == Man(0, 0)
DataFiles == {F0} \cup {Data(w) : w \in Writers}
UpFile(w) == IF OpOf[w] = "delete" THEN Del(w) ELSE Data(w)
Owner(f) == f[2]

PCs == {"init", "planned", "uploading", "uploaded", "built", "waiting",
        "conflicted", "unknown", "done", "aborted", "failed", "crashed"}
Terminal == {"done", "aborted", "failed", "crashed"}

\* A snapshot: id = <<writer, attempt>>; w = the operation token (summary property);
\* files = live data files; del = position deletes (file, row) pairs; dfiles = delete files
\* referenced; man = manifest list. addedRows / delPairs = what this snapshot changed (delPairs:
\* the pairs newly deleted; it feeds the ghost history). delTargets = the (file, row) pairs the
\* delete file this snapshot added NAMES, including pairs that were already deleted (a redundant
\* delete): it decides which data files the delete file applies to (compaction validation).
S0 == [id |-> <<0, 0>>, parent |-> <<0, 0>>, op |-> "init", w |-> 0, seq |-> 0,
       files |-> {F0}, del |-> {}, dfiles |-> {}, man |-> M0,
       addedRows |-> InitRows, delPairs |-> {}, delTargets |-> {}]
Meta0 == [ver |-> 0, lastSeq |-> 0, snaps |-> {S0}, cur |-> S0]
NoMeta == [ver |-> -1, lastSeq |-> -1, snaps |-> {}, cur |-> S0]

CurVer == CHOOSE v \in DOMAIN mlog : \A u \in DOMAIN mlog : u <= v
CurMeta == mlog[CurVer]

Refs(s) == s.files \cup s.dfiles \cup {s.man}
LiveRows(s) == {r \in Rows : \E f \in s.files : r \in frows[f] /\ <<f, r>> \notin s.del}
ReadRows(s) == {r \in Rows : \E f \in s.files : f \in durable /\ r \in frows[f] /\ <<f, r>> \notin s.del}

(* ------------------------------------------------------------------------ *)
(* What a writer plans and builds                                           *)
(* ------------------------------------------------------------------------ *)
PlanFor(w, b, v) ==
    [ver     |-> IF OpOf[w] = "compact" THEN v ELSE 0,   \* the version the plan was read at (compaction only: I8)
     targets |-> IF OpOf[w] = "delete"
                   THEN {p \in b.files \X Rows :
                            p[2] \in frows[p[1]] /\ KeyOf[p[2]] = DelKey[w] /\ p \notin b.del}
                   ELSE {},
     inputs  |-> IF OpOf[w] = "compact" THEN b.files ELSE {}]

ContentFor(w, b) ==
    CASE OpOf[w] = "append"  -> AppendRows[w]
      [] OpOf[w] = "compact" -> {r \in Rows : \E f \in b.files : r \in frows[f] /\ <<f, r>> \notin b.del}
      [] OpOf[w] = "delete"  -> {}

NextSeq(bm) == bm.lastSeq + 1                                   \* MUTATION POINT: sequence number

BuildSnap(w, k) ==
    LET bm == mlog[base[w]]
        b  == bm.cur
        pl == plan[w]
    IN  CASE OpOf[w] = "append" ->
              [id |-> <<w, k>>, parent |-> b.id, op |-> "append", w |-> w, seq |-> NextSeq(bm),
               files |-> b.files \cup {Data(w)}, del |-> b.del, dfiles |-> b.dfiles,
               man |-> Man(w, k), addedRows |-> AppendRows[w], delPairs |-> {}, delTargets |-> {}]
          [] OpOf[w] = "delete" ->
              [id |-> <<w, k>>, parent |-> b.id, op |-> "delete", w |-> w, seq |-> NextSeq(bm),
               files |-> b.files, del |-> b.del \cup pl.targets, dfiles |-> b.dfiles \cup {Del(w)},
               man |-> Man(w, k), addedRows |-> {}, delPairs |-> pl.targets \ b.del,
               delTargets |-> pl.targets]
          [] OpOf[w] = "compact" ->
              [id |-> <<w, k>>, parent |-> b.id, op |-> "compact", w |-> w, seq |-> NextSeq(bm),
               files |-> (b.files \ pl.inputs) \cup {Data(w)},
               del |-> {p \in b.del : p[1] \notin pl.inputs}, dfiles |-> b.dfiles,
               man |-> Man(w, k), addedRows |-> {}, delPairs |-> {}, delTargets |-> {}]

\* The metadata a STORE writer puts at the next version (REST: a placeholder, the server builds it).
FullMeta(w, s) ==
    IF Style = "STORE"
       THEN [ver |-> exp[w] + 1, lastSeq |-> s.seq, snaps |-> mlog[base[w]].snaps \cup {s}, cur |-> s]
       ELSE NoMeta

(* ------------------------------------------------------------------------ *)
(* Conflict validation on retry (Iceberg: "Commit Conflict Resolution and   *)
(* Retry"; serializable vs snapshot isolation is a table property).         *)
(* ------------------------------------------------------------------------ *)
\* A compaction conflicts with ANY delete file committed after its base that names one of its input
\* data files, even a redundant one that deletes no row that was not already deleted (Iceberg's
\* validateNoNewDeletesForDataFiles asks whether a delete file applies to a file being rewritten,
\* not whether it removes new rows). Stricter than "no newly deleted pair": it may abort a
\* compaction needlessly, never lets one lose or resurrect a row.
CompactionSeesNoDeleteFile(w, inter) ==                          \* MUTATION POINT: compaction vs delete files
    \A s \in inter : \A p \in s.delTargets : p[1] \notin plan[w].inputs

Valid(w, cm) ==
    LET inter == cm.snaps \ mlog[base[w]].snaps      \* snapshots committed since my base
    IN  CASE OpOf[w] = "append" -> TRUE
          [] OpOf[w] = "delete" ->
               ( (\A p \in plan[w].targets : p[1] \in cm.cur.files)
                 /\ (Isolation = "serializable" =>
                       ~(\E s \in inter : \E r \in s.addedRows : KeyOf[r] = DelKey[w])) )
          [] OpOf[w] = "compact" ->
               ( (plan[w].inputs \subseteq cm.cur.files)
                 /\ CompactionSeesNoDeleteFile(w, inter) )

Validated(w) == Valid(w, CurMeta)                                \* MUTATION POINT: conflict validation
Mine(w) == \E s \in CurMeta.snaps : s.w = w                      \* MUTATION POINT: look for my own snapshot (token)
NewBaseOnRetry(w) == CurVer                                      \* MUTATION POINT: base after a conflict
UploadDone(w) == pc[w] = "uploaded"                              \* MUTATION POINT: commit only after the upload is durable
PurgeOnAck(w) == {}                                              \* MUTATION POINT: files deleted when the commit is acknowledged
CasOk(q) == CurVer = q.exp                                       \* MUTATION POINT: the compare-and-swap
GCOwnerDead(f) == pc[Owner(f)] \in Terminal                      \* MUTATION POINT: GC leaves active writers' files alone
GCNoRequest(f) == \A q \in net : f \notin Refs(q.snap)           \* MUTATION POINT: GC leaves in-flight requests' files alone

(* ------------------------------------------------------------------------ *)
(* Initial state                                                            *)
(* ------------------------------------------------------------------------ *)
Init ==
    /\ mlog = (0 :> Meta0)
    /\ hist = <<>>
    /\ durable = {F0, M0}
    /\ inflight = {}
    /\ frows = [f \in DataFiles |-> IF f = F0 THEN InitRows ELSE {}]
    /\ net = {}
    /\ resp = [w \in Writers |-> "none"]
    /\ pc = [w \in Writers |-> "init"]
    /\ att = [w \in Writers |-> 0]
    /\ base = [w \in Writers |-> 0]
    /\ exp = [w \in Writers |-> 0]
    /\ plan = [w \in Writers |-> [ver |-> 0, targets |-> {}, inputs |-> {}]]
    /\ snap = [w \in Writers |-> S0]
    /\ pinned = {}
    /\ crashLeft = CrashBudget
    /\ dupLeft = DupBudget
    /\ lossLeft = LossBudget

(* ------------------------------------------------------------------------ *)
(* Writer steps                                                             *)
(* ------------------------------------------------------------------------ *)
PlanOp(w) ==
    /\ pc[w] = "init"
    /\ base' = [base EXCEPT ![w] = CurVer]
    /\ exp' = [exp EXCEPT ![w] = CurVer]
    /\ plan' = [plan EXCEPT ![w] = PlanFor(w, CurMeta.cur, CurVer)]
    /\ frows' = IF OpOf[w] = "delete" THEN frows
                ELSE [frows EXCEPT ![Data(w)] = ContentFor(w, CurMeta.cur)]
    /\ att' = [att EXCEPT ![w] = 1]
    /\ pc' = [pc EXCEPT ![w] = "planned"]
    /\ UNCHANGED <<ledger, durable, inflight, netv, snap, pinned, bud>>

BeginUpload(w) ==
    /\ pc[w] = "planned"
    /\ inflight' = inflight \cup {UpFile(w)}
    /\ pc' = [pc EXCEPT ![w] = "uploading"]
    /\ UNCHANGED <<ledger, durable, frows, netv, att, base, exp, plan, snap, pinned, bud>>

\* An upload completes atomically (object-store PUT); the writer need not still be alive.
FinishUpload(f) ==
    /\ f \in inflight
    /\ durable' = durable \cup {f}
    /\ inflight' = inflight \ {f}
    /\ UNCHANGED <<ledger, frows, netv, wl, pinned, bud>>

AwaitUpload(w) ==
    /\ pc[w] = "uploading"
    /\ UpFile(w) \in durable
    /\ pc' = [pc EXCEPT ![w] = "uploaded"]
    /\ UNCHANGED <<ledger, filev, netv, att, base, exp, plan, snap, pinned, bud>>

WriteMeta(w) ==
    /\ UploadDone(w)
    /\ durable' = durable \cup {Man(w, att[w])}
    /\ snap' = [snap EXCEPT ![w] = BuildSnap(w, att[w])]
    /\ pc' = [pc EXCEPT ![w] = "built"]
    /\ UNCHANGED <<ledger, inflight, frows, netv, att, base, exp, plan, pinned, bud>>

Send(w) ==
    /\ pc[w] = "built"
    /\ net' = net \cup {[w |-> w, k |-> att[w], exp |-> exp[w], snap |-> snap[w],
                         meta |-> FullMeta(w, snap[w])]}
    /\ resp' = [resp EXCEPT ![w] = "none"]
    /\ pc' = [pc EXCEPT ![w] = "waiting"]
    /\ UNCHANGED <<ledger, filev, att, base, exp, plan, snap, pinned, bud>>

Receive(w) ==
    /\ pc[w] = "waiting"
    /\ resp[w] # "none"
    /\ pc' = [pc EXCEPT ![w] = IF resp[w] = "ok" THEN "done" ELSE "conflicted"]
    /\ durable' = durable \ (IF resp[w] = "ok" THEN PurgeOnAck(w) ELSE {})
    /\ resp' = [resp EXCEPT ![w] = "none"]
    /\ UNCHANGED <<ledger, inflight, frows, net, att, base, exp, plan, snap, pinned, bud>>

\* CommitStateUnknown: a gateway timeout / 500 / 502 at any moment while waiting.
Timeout(w) ==
    /\ pc[w] = "waiting"
    /\ pc' = [pc EXCEPT ![w] = "unknown"]
    /\ resp' = [resp EXCEPT ![w] = "none"]
    /\ UNCHANGED <<ledger, filev, net, att, base, exp, plan, snap, pinned, bud>>

\* After 409 or after an unknown outcome: re-read, look for my snapshot, validate, rebuild, retry.
RecoverDone(w) ==
    /\ pc[w] \in {"conflicted", "unknown"}
    /\ Mine(w)
    /\ pc' = [pc EXCEPT ![w] = "done"]
    /\ UNCHANGED <<ledger, filev, netv, att, base, exp, plan, snap, pinned, bud>>

RecoverAbort(w) ==
    /\ pc[w] \in {"conflicted", "unknown"}
    /\ ~Mine(w)
    /\ ~Validated(w)
    /\ pc' = [pc EXCEPT ![w] = "aborted"]
    /\ UNCHANGED <<ledger, filev, netv, att, base, exp, plan, snap, pinned, bud>>

RecoverFail(w) ==
    /\ pc[w] \in {"conflicted", "unknown"}
    /\ ~Mine(w)
    /\ Validated(w)
    /\ att[w] >= MaxAttempts
    /\ pc' = [pc EXCEPT ![w] = "failed"]
    /\ UNCHANGED <<ledger, filev, netv, att, base, exp, plan, snap, pinned, bud>>

RecoverRetry(w) ==
    /\ pc[w] \in {"conflicted", "unknown"}
    /\ ~Mine(w)
    /\ Validated(w)
    /\ att[w] < MaxAttempts
    /\ att' = [att EXCEPT ![w] = att[w] + 1]
    /\ base' = [base EXCEPT ![w] = NewBaseOnRetry(w)]
    /\ exp' = [exp EXCEPT ![w] = CurVer]
    /\ pc' = [pc EXCEPT ![w] = "uploaded"]
    /\ UNCHANGED <<ledger, filev, netv, plan, snap, pinned, bud>>

Crash(w) ==
    /\ crashLeft > 0
    /\ pc[w] \notin Terminal
    /\ pc' = [pc EXCEPT ![w] = "crashed"]
    /\ crashLeft' = crashLeft - 1
    /\ UNCHANGED <<ledger, filev, netv, att, base, exp, plan, snap, pinned, dupLeft, lossLeft>>

(* ------------------------------------------------------------------------ *)
(* The catalog / store, the network, the environment                        *)
(* ------------------------------------------------------------------------ *)
TargetVer(q) == IF Style = "REST" THEN CurVer + 1 ELSE q.meta.ver
NewMeta(q) ==
    IF Style = "REST"
       THEN [ver |-> CurVer + 1, lastSeq |-> q.snap.seq, snaps |-> CurMeta.snaps \cup {q.snap}, cur |-> q.snap]
       ELSE q.meta
Answer(q, res) ==
    resp' = IF pc[q.w] = "waiting" /\ att[q.w] = q.k THEN [resp EXCEPT ![q.w] = res] ELSE resp

\* keep = TRUE: the network delivers this request a second time later (duplicate).
DeliverOk(q, keep) ==
    /\ q \in net
    /\ keep => dupLeft > 0
    /\ CasOk(q)
    /\ TargetVer(q) <= MaxVer
    /\ mlog' = (TargetVer(q) :> NewMeta(q)) @@ mlog
    /\ hist' = Append(hist, [w |-> q.snap.w, addedRows |-> q.snap.addedRows,
                             delRows |-> {p[2] : p \in q.snap.delPairs}])
    /\ Answer(q, "ok")
    /\ net' = IF keep THEN net ELSE net \ {q}
    /\ dupLeft' = IF keep THEN dupLeft - 1 ELSE dupLeft
    /\ UNCHANGED <<filev, wl, pinned, crashLeft, lossLeft>>

DeliverConflict(q, keep) ==
    /\ q \in net
    /\ keep => dupLeft > 0
    /\ ~CasOk(q)
    /\ Answer(q, "conflict")
    /\ net' = IF keep THEN net ELSE net \ {q}
    /\ dupLeft' = IF keep THEN dupLeft - 1 ELSE dupLeft
    /\ UNCHANGED <<ledger, filev, wl, pinned, crashLeft, lossLeft>>

Drop(q) ==
    /\ q \in net
    /\ lossLeft > 0
    /\ net' = net \ {q}
    /\ lossLeft' = lossLeft - 1
    /\ UNCHANGED <<ledger, filev, resp, wl, pinned, crashLeft, dupLeft>>

\* Orphan collection. Eligible: referenced by no snapshot, its writer is finished, and no
\* request in flight refers to it (assumption A-GC).
GCOrphan(f) ==
    /\ f \in durable
    /\ f # F0 /\ f # M0
    /\ ~(\E s \in CurMeta.snaps : f \in Refs(s))
    /\ GCOwnerDead(f)
    /\ GCNoRequest(f)
    /\ durable' = durable \ {f}
    /\ UNCHANGED <<ledger, inflight, frows, netv, wl, pinned, bud>>

PinReader ==
    /\ Cardinality(pinned) < MaxPins
    /\ pinned' = pinned \cup {[id |-> CurMeta.cur.id, rows |-> ReadRows(CurMeta.cur)]}
    /\ UNCHANGED <<ledger, filev, netv, wl, bud>>

WriterStep(w) ==
    \/ PlanOp(w) \/ BeginUpload(w) \/ AwaitUpload(w) \/ WriteMeta(w) \/ Send(w)
    \/ Receive(w) \/ Timeout(w)
    \/ RecoverDone(w) \/ RecoverAbort(w) \/ RecoverFail(w) \/ RecoverRetry(w)

Deliver(q) == \/ DeliverOk(q, FALSE) \/ DeliverOk(q, TRUE)
              \/ DeliverConflict(q, FALSE) \/ DeliverConflict(q, TRUE)

Next ==
    \/ \E w \in Writers : WriterStep(w) \/ Crash(w)
    \/ \E f \in inflight : FinishUpload(f)
    \/ \E q \in net : Deliver(q) \/ Drop(q)
    \/ \E f \in durable : GCOrphan(f)
    \/ PinReader

Fairness ==
    /\ \A w \in Writers : WF_vars(WriterStep(w))
    /\ WF_vars(\E q \in net : Deliver(q))
    /\ WF_vars(\E f \in inflight : FinishUpload(f))

Spec == Init /\ [][Next]_vars /\ Fairness

(* ------------------------------------------------------------------------ *)
(* Invariants                                                               *)
(* ------------------------------------------------------------------------ *)
TypeOK ==
    /\ \A w \in Writers : pc[w] \in PCs /\ att[w] \in 0..MaxAttempts /\ resp[w] \in {"none", "ok", "conflict"}
    /\ crashLeft \in 0..CrashBudget /\ dupLeft \in 0..DupBudget /\ lossLeft \in 0..LossBudget

HistSet == {hist[i] : i \in 1..Len(hist)}
ExpectedRows == (InitRows \cup UNION {h.addedRows : h \in HistSet}) \ UNION {h.delRows : h \in HistSet}

\* I1: the catalog never points to metadata that references a missing file. Every snapshot in
\* the current metadata (all are "valid snapshots" in the spec's sense) has all its files.
I1_FilesExist == \A s \in CurMeta.snaps : Refs(s) \subseteq durable

\* I2: no lost update. The rows of the current snapshot are exactly the rows added by all
\* successful commits minus the rows deleted by all successful commits (GHOST history).
I2_NoLostUpdate == LiveRows(CurMeta.cur) = ExpectedRows

\* I3: snapshot isolation of readers. A pinned reader still finds its snapshot unchanged
\* and every file it needs, and sees exactly the rows it saw.
I3_ReaderSeesSnapshot ==
    \A p \in pinned : \E s \in CurMeta.snaps : s.id = p.id /\ ReadRows(s) = p.rows

\* I4: serializable. A delete-where commits a snapshot in which no row matching its
\* predicate is live (as if it had run last). Holds under "serializable" only.
I4_NoPhantom ==
    \A s \in CurMeta.snaps : s.op = "delete" => \A r \in LiveRows(s) : KeyOf[r] # DelKey[s.w]

\* I5: an operation commits at most once (retry after an unknown outcome is not a duplicate).
I5_AtMostOnce == \A w \in Writers : Cardinality({s \in CurMeta.snaps : s.w = w}) <= 1

\* I6: sequence numbers strictly increase with the version, and are unique.
I6_SeqIncreases ==
    \A v \in DOMAIN mlog :
        v > 0 => ( mlog[v].lastSeq > mlog[v-1].lastSeq
                   /\ \A s, t \in mlog[v].snaps : s # t => s.seq # t.seq )

\* I7: history is linear and nothing committed is lost: each version keeps all snapshots of
\* the previous one and its new snapshot is a child of the previous current snapshot.
I7_NoSnapshotLost ==
    \A v \in DOMAIN mlog :
        v > 0 => ( mlog[v-1].snaps \subseteq mlog[v].snaps
                   /\ mlog[v].cur.parent = mlog[v-1].cur.id )

\* I8: a compaction commits only if no delete file naming one of its input files was committed
\* between its plan and its own commit, whether or not that file deleted new rows (the rule of
\* CompactionSeesNoDeleteFile, stated on the history of versions instead of on the validation
\* step: plan[w].ver is the version the plan was read at). Not a safety property of the table (a
\* looser rule loses no row: I2 holds under it); it pins the conservative rule of the code and of
\* Iceberg. It can only be violated with two deleters and a compaction (cfg ddc-rest).
I8_CompactionSawNoDeleteFile ==
    \A v \in DOMAIN mlog :
        (v > 0 /\ mlog[v].cur.op = "compact") =>
            LET w == mlog[v].cur.w IN
            \A u \in DOMAIN mlog :
                (u > plan[w].ver /\ u < v) =>
                    \A p \in mlog[u].cur.delTargets : p[1] \notin plan[w].inputs

(* ------------------------------------------------------------------------ *)
(* Liveness (needs the fairness in Spec)                                    *)
(* ------------------------------------------------------------------------ *)
\* With bounded retries and fair scheduling every writer ends.
Termination == <>(\A w \in Writers : pc[w] \in Terminal)
\* Some writer makes progress: a commit lands, or every writer crashed.
Progress == (<>(Cardinality(DOMAIN mlog) > 1)) \/ (<>(\A w \in Writers : pc[w] = "crashed"))

(* ------------------------------------------------------------------------ *)
(* Coverage canaries: each is meant to be VIOLATED, which proves the        *)
(* situation is reachable in the configuration (no vacuous pass).           *)
(* ------------------------------------------------------------------------ *)
Canary_Unknown == ~(\E w \in Writers : pc[w] = "unknown")
Canary_Conflict == ~(\E w \in Writers : pc[w] = "conflicted")
Canary_RetryCommitted == ~(\E s \in CurMeta.snaps : s.id[2] >= 2)
Canary_CompactCommitted == ~(\E s \in CurMeta.snaps : s.op = "compact")
Canary_DeleteCommitted == ~(\E s \in CurMeta.snaps : s.op = "delete")
Canary_ValidationAbort == ~(\E w \in Writers : pc[w] = "aborted")
Canary_CrashedButCommitted == ~(\E w \in Writers : pc[w] = "crashed" /\ \E s \in CurMeta.snaps : s.w = w)
Canary_LateOldAttempt == ~(\E w \in Writers : \E s \in CurMeta.snaps : s.w = w /\ s.id[2] < att[w])
Canary_DuplicateDelivered == dupLeft = DupBudget
\* attempt 1's manifest list was written (the writer is on attempt 2) and has since been collected
Canary_OrphanCollected == ~(\E w \in Writers : att[w] >= 2 /\ Man(w, 1) \notin durable)
Canary_ReaderPinned == pinned = {}
=============================================================================
