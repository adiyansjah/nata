------------------------------ MODULE QueryLifecycle ------------------------------
(* Failure model of ONE distributed query (verification-plan item 29). Throwaway spike.

   Roles. One coordinator, a set of workers, a set of tasks. Each task runs on one worker and writes its output to that
   worker's local disk (a "file"); the coordinator commits ONE attempt per task (fencing by attempt number and worker),
   then reads ("consumes") the committed output exactly once. The query succeeds when every task has been consumed.

   Network. `net` is a SET of messages that is never emptied by receiving: every handler may fire again later on the same
   message (duplication, arbitrary delay and reordering). A message can also be lost (budgeted).

   Failures. A worker may crash for good (its disk is gone). The coordinator may suspect a worker, rightly or wrongly (a wrong
   suspicion makes a ZOMBIE: it keeps running and its late results arrive). The coordinator may crash for good.

   THE RULES THIS MODEL CHECKS (the proposed answer to "what does a failed X do"):
     failed TASK/WORKER  the coordinator re-runs the task under a NEW attempt number on another worker, at most MaxAttempts times,
                         then fails the query (also when no unsuspected worker is left: found by TLC); a late result of an old attempt or from the wrong worker is rejected;
     a task whose output was ALREADY consumed is never re-run (the consumed result stays valid even if its worker dies later);
     failed COORDINATOR  the query fails (no recovery in v1); every worker releases its tasks and files when its lease expires;
     CANCEL / end        the coordinator sends `release` to every worker (also suspected ones) until each one acknowledges;
     the client          can never be told "succeeded" unless the query succeeded.
   Modelling limits (honest): consumption of a task's output is atomic (no half-read stream); tasks are deterministic (a re-run
   produces the same output); the disk of a crashed worker is lost, a crashed worker never returns. *)
EXTENDS Naturals, FiniteSets

CONSTANTS Tasks, Workers, NoWorker,
          MaxAttempts,       \* attempts per task
          CrashBudget,       \* worker crashes
          SuspectBudget,     \* wrong suspicions (suspecting a crashed worker is free)
          LossBudget,        \* messages lost
          CoordCrashBudget   \* 0 or 1

VARIABLES ran, qs, coordUp, client, tstate, attempt, assign, commit, commitW, consumed, dups,
          suspect, wUp, running, files, released, cleaned, net, crashes, susp, lost, ccrash
vars == <<ran, qs, coordUp, client, tstate, attempt, assign, commit, commitW, consumed, dups,
          suspect, wUp, running, files, released, cleaned, net, crashes, susp, lost, ccrash>>

Terminal == {"succeeded", "failed", "cancelled"}
QOver == qs \in Terminal \/ ~coordUp
Att == 1..MaxAttempts
Fl(t, a) == [t |-> t, a |-> a]

MStart(t, a, w) == [k |-> "start",   t |-> t, a |-> a, w |-> w]
MDone(t, a, w)  == [k |-> "done",    t |-> t, a |-> a, w |-> w]
MDisc(t, a, w)  == [k |-> "discard", t |-> t, a |-> a, w |-> w]
MRel(w)         == [k |-> "release", w |-> w]
MAck(w)         == [k |-> "ack",     w |-> w]

(* ---- rule hooks: the mutants edit exactly these lines ---- *)
CanRetry(t)         == attempt[t] < MaxAttempts
Invalidate(t)       == tstate[t] = "running" \/ (tstate[t] = "done" /\ consumed[t] = 0)
AcceptOpen          == qs = "running"
Fenced(m)           == m.a = attempt[m.t] /\ m.w = assign[m.t]
CanConsume(t)       == consumed[t] = 0
AllConsumed         == \A t \in Tasks : consumed[t] /= 0
NoWorkerLeft       == suspect = Workers   \* found by TLC: without this clause the query hangs when every worker is suspected
StartFresh(m)       == Fl(m.t, m.a) \notin ran[m.w]
ReleaseTarget(w)    == TRUE
CleanedAfterSend(w) == {}
LeaseOn             == ~coordUp
ResendOn            == TRUE
(* ---- end of hooks ---- *)

Init ==
  /\ qs = "running" /\ coordUp = TRUE /\ client = "waiting"
  /\ tstate = [t \in Tasks |-> "pending"] /\ attempt = [t \in Tasks |-> 0]
  /\ assign = [t \in Tasks |-> NoWorker] /\ commit = [t \in Tasks |-> 0] /\ commitW = [t \in Tasks |-> NoWorker]
  /\ consumed = [t \in Tasks |-> 0] /\ dups = 0
  /\ suspect = {} /\ wUp = Workers
  /\ running = [w \in Workers |-> {}] /\ files = [w \in Workers |-> {}]
  /\ released = {} /\ cleaned = {} /\ net = {} /\ ran = [w \in Workers |-> {}]
  /\ crashes = 0 /\ susp = 0 /\ lost = 0 /\ ccrash = 0

(* --- coordinator --- *)
Schedule(t, w) ==
  /\ coordUp /\ qs = "running" /\ tstate[t] = "pending" /\ CanRetry(t) /\ w \notin suspect
  /\ attempt' = [attempt EXCEPT ![t] = @ + 1]
  /\ assign'  = [assign  EXCEPT ![t] = w]
  /\ tstate'  = [tstate  EXCEPT ![t] = "running"]
  /\ net' = net \cup {MStart(t, attempt[t] + 1, w)}
  /\ UNCHANGED <<ran, qs, coordUp, client, commit, commitW, consumed, dups, suspect, wUp, running, files, released,
                 cleaned, crashes, susp, lost, ccrash>>

CoordAccept(m) ==
  /\ m.k = "done" /\ coordUp /\ AcceptOpen /\ tstate[m.t] = "running" /\ commit[m.t] = 0 /\ Fenced(m)
  /\ commit'  = [commit  EXCEPT ![m.t] = m.a]
  /\ commitW' = [commitW EXCEPT ![m.t] = m.w]
  /\ tstate'  = [tstate  EXCEPT ![m.t] = "done"]
  /\ UNCHANGED <<ran, qs, coordUp, client, attempt, assign, consumed, dups, suspect, wUp, running, files, released,
                 cleaned, net, crashes, susp, lost, ccrash>>

\* a result that is not the committed one is rejected and its file is discarded
CoordReject(m) ==
  /\ m.k = "done" /\ coordUp /\ qs = "running"
  /\ ~(commit[m.t] = m.a /\ commitW[m.t] = m.w)
  /\ ~(tstate[m.t] = "running" /\ commit[m.t] = 0 /\ Fenced(m))
  /\ MDisc(m.t, m.a, m.w) \notin net
  /\ net' = net \cup {MDisc(m.t, m.a, m.w)}
  /\ UNCHANGED <<ran, qs, coordUp, client, tstate, attempt, assign, commit, commitW, consumed, dups, suspect, wUp, running,
                 files, released, cleaned, crashes, susp, lost, ccrash>>

\* a `start` can be lost: the coordinator keeps re-sending it while the attempt is outstanding (found by TLC)
ResendStart(t) ==
  /\ coordUp /\ qs = "running" /\ ResendOn /\ tstate[t] = "running"
  /\ MStart(t, attempt[t], assign[t]) \notin net
  /\ net' = net \cup {MStart(t, attempt[t], assign[t])}
  /\ UNCHANGED <<ran, qs, coordUp, client, tstate, attempt, assign, commit, commitW, consumed, dups, suspect, wUp, running,
                 files, released, cleaned, crashes, susp, lost, ccrash>>

Suspect(w) ==
  /\ coordUp /\ qs = "running" /\ w \notin suspect
  /\ (w \notin wUp \/ susp < SuspectBudget)
  /\ suspect' = suspect \cup {w}
  /\ susp' = IF w \in wUp THEN susp + 1 ELSE susp
  /\ LET inv == {t \in Tasks : assign[t] = w /\ Invalidate(t)} IN
     /\ tstate'  = [t \in Tasks |-> IF t \in inv THEN "pending" ELSE tstate[t]]
     /\ commit'  = [t \in Tasks |-> IF t \in inv THEN 0 ELSE commit[t]]
     /\ commitW' = [t \in Tasks |-> IF t \in inv THEN NoWorker ELSE commitW[t]]
     /\ assign'  = [t \in Tasks |-> IF t \in inv THEN NoWorker ELSE assign[t]]
  /\ UNCHANGED <<ran, qs, coordUp, client, attempt, consumed, dups, wUp, running, files, released, cleaned, net,
                 crashes, lost, ccrash>>

Consume(t) ==
  /\ coordUp /\ qs = "running" /\ tstate[t] = "done" /\ CanConsume(t)
  /\ commitW[t] \in wUp /\ Fl(t, commit[t]) \in files[commitW[t]]
  /\ consumed' = [consumed EXCEPT ![t] = commit[t]]
  /\ dups' = IF consumed[t] /= 0 THEN dups + 1 ELSE dups
  /\ UNCHANGED <<ran, qs, coordUp, client, tstate, attempt, assign, commit, commitW, suspect, wUp, running, files, released,
                 cleaned, net, crashes, susp, lost, ccrash>>

Succeed ==
  /\ coordUp /\ qs = "running" /\ AllConsumed
  /\ qs' = "succeeded"
  /\ UNCHANGED <<ran, coordUp, client, tstate, attempt, assign, commit, commitW, consumed, dups, suspect, wUp, running, files,
                 released, cleaned, net, crashes, susp, lost, ccrash>>

FailQuery ==
  /\ coordUp /\ qs = "running"
  /\ \E t \in Tasks : tstate[t] = "pending" /\ consumed[t] = 0 /\ (~CanRetry(t) \/ NoWorkerLeft)
  /\ qs' = "failed"
  /\ UNCHANGED <<ran, coordUp, client, tstate, attempt, assign, commit, commitW, consumed, dups, suspect, wUp, running, files,
                 released, cleaned, net, crashes, susp, lost, ccrash>>

Cancel ==
  /\ coordUp /\ qs = "running"
  /\ qs' = "cancelled"
  /\ UNCHANGED <<ran, coordUp, client, tstate, attempt, assign, commit, commitW, consumed, dups, suspect, wUp, running, files,
                 released, cleaned, net, crashes, susp, lost, ccrash>>

\* after the end, keep sending `release` to every worker until it acknowledges
ReleaseSend(w) ==
  /\ coordUp /\ qs \in Terminal /\ w \notin cleaned /\ ReleaseTarget(w) /\ MRel(w) \notin net
  /\ net' = net \cup {MRel(w)}
  /\ cleaned' = cleaned \cup CleanedAfterSend(w)
  /\ UNCHANGED <<ran, qs, coordUp, client, tstate, attempt, assign, commit, commitW, consumed, dups, suspect, wUp, running,
                 files, released, crashes, susp, lost, ccrash>>

CoordAck(m) ==
  /\ m.k = "ack" /\ coordUp /\ m.w \notin cleaned
  /\ cleaned' = cleaned \cup {m.w}
  /\ UNCHANGED <<ran, qs, coordUp, client, tstate, attempt, assign, commit, commitW, consumed, dups, suspect, wUp, running,
                 files, released, net, crashes, susp, lost, ccrash>>

CoordCrash ==
  /\ coordUp /\ ccrash < CoordCrashBudget
  /\ coordUp' = FALSE /\ ccrash' = ccrash + 1
  /\ UNCHANGED <<ran, qs, client, tstate, attempt, assign, commit, commitW, consumed, dups, suspect, wUp, running, files,
                 released, cleaned, net, crashes, susp, lost>>

(* --- client --- *)
ClientObserve ==
  /\ coordUp /\ qs \in Terminal /\ client = "waiting"
  /\ client' = qs
  /\ UNCHANGED <<ran, qs, coordUp, tstate, attempt, assign, commit, commitW, consumed, dups, suspect, wUp, running, files,
                 released, cleaned, net, crashes, susp, lost, ccrash>>

ClientLost ==
  /\ ~coordUp /\ client = "waiting"
  /\ client' = "failed"
  /\ UNCHANGED <<ran, qs, coordUp, tstate, attempt, assign, commit, commitW, consumed, dups, suspect, wUp, running, files,
                 released, cleaned, net, crashes, susp, lost, ccrash>>

(* --- workers --- *)
\* a worker runs each attempt id at most once: a duplicate or late `start` of an attempt it already ran is ignored
WorkerStart(m) ==
  /\ m.k = "start" /\ m.w \in wUp /\ m.w \notin released /\ StartFresh(m)
  /\ running' = [running EXCEPT ![m.w] = @ \cup {Fl(m.t, m.a)}]
  /\ ran' = [ran EXCEPT ![m.w] = @ \cup {Fl(m.t, m.a)}]
  /\ UNCHANGED <<qs, coordUp, client, tstate, attempt, assign, commit, commitW, consumed, dups, suspect, wUp, files,
                 released, cleaned, net, crashes, susp, lost, ccrash>>

WorkerFinish(w, r) ==
  /\ w \in wUp /\ r \in running[w]
  /\ running' = [running EXCEPT ![w] = @ \ {r}]
  /\ files'   = [files   EXCEPT ![w] = @ \cup {r}]
  /\ net' = net \cup {MDone(r.t, r.a, w)}
  /\ UNCHANGED <<ran, qs, coordUp, client, tstate, attempt, assign, commit, commitW, consumed, dups, suspect, wUp, released,
                 cleaned, crashes, susp, lost, ccrash>>

\* a `done` can be lost: the worker keeps re-sending it while it holds the file (found by TLC)
WorkerResend(w, r) ==
  /\ w \in wUp /\ ResendOn /\ r \in files[w] /\ MDone(r.t, r.a, w) \notin net
  /\ net' = net \cup {MDone(r.t, r.a, w)}
  /\ UNCHANGED <<ran, qs, coordUp, client, tstate, attempt, assign, commit, commitW, consumed, dups, suspect, wUp, running,
                 files, released, cleaned, crashes, susp, lost, ccrash>>

WorkerDiscard(m) ==
  /\ m.k = "discard" /\ m.w \in wUp /\ Fl(m.t, m.a) \in files[m.w]
  /\ files' = [files EXCEPT ![m.w] = @ \ {Fl(m.t, m.a)}]
  /\ UNCHANGED <<ran, qs, coordUp, client, tstate, attempt, assign, commit, commitW, consumed, dups, suspect, wUp, running,
                 released, cleaned, net, crashes, susp, lost, ccrash>>

\* a worker that releases drops its tasks and files and refuses new starts of this query
Drop(w) ==
  /\ running' = [running EXCEPT ![w] = {}]
  /\ files'   = [files   EXCEPT ![w] = {}]
  /\ released' = released \cup {w}

WorkerRelease(m) ==
  /\ m.k = "release" /\ m.w \in wUp
  /\ (running[m.w] /= {} \/ files[m.w] /= {} \/ m.w \notin released \/ MAck(m.w) \notin net)
  /\ Drop(m.w)
  /\ net' = net \cup {MAck(m.w)}
  /\ UNCHANGED <<ran, qs, coordUp, client, tstate, attempt, assign, commit, commitW, consumed, dups, suspect, wUp,
                 cleaned, crashes, susp, lost, ccrash>>

LeaseExpire(w) ==
  /\ w \in wUp /\ LeaseOn
  /\ (running[w] /= {} \/ files[w] /= {} \/ w \notin released)
  /\ Drop(w)
  /\ UNCHANGED <<ran, qs, coordUp, client, tstate, attempt, assign, commit, commitW, consumed, dups, suspect, wUp,
                 cleaned, net, crashes, susp, lost, ccrash>>

(* --- environment --- *)
WorkerCrash(w) ==
  /\ w \in wUp /\ crashes < CrashBudget
  /\ wUp' = wUp \ {w} /\ crashes' = crashes + 1
  /\ running' = [running EXCEPT ![w] = {}]
  /\ files'   = [files   EXCEPT ![w] = {}]
  /\ UNCHANGED <<ran, qs, coordUp, client, tstate, attempt, assign, commit, commitW, consumed, dups, suspect, released,
                 cleaned, net, susp, lost, ccrash>>

Lose(m) ==
  /\ lost < LossBudget
  /\ net' = net \ {m} /\ lost' = lost + 1
  /\ UNCHANGED <<ran, qs, coordUp, client, tstate, attempt, assign, commit, commitW, consumed, dups, suspect, wUp, running,
                 files, released, cleaned, crashes, susp, ccrash>>

Next ==
  \/ \E t \in Tasks, w \in Workers : Schedule(t, w)
  \/ \E m \in net : CoordAccept(m) \/ CoordReject(m) \/ CoordAck(m) \/ WorkerStart(m) \/ WorkerDiscard(m)
                    \/ WorkerRelease(m) \/ Lose(m)
  \/ \E w \in Workers : Suspect(w) \/ ReleaseSend(w) \/ LeaseExpire(w) \/ WorkerCrash(w)
  \/ \E w \in Workers : \E r \in running[w] : WorkerFinish(w, r)
  \/ \E w \in Workers : \E r \in files[w] : WorkerResend(w, r)
  \/ \E t \in Tasks : ResendStart(t) \/ Consume(t)
  \/ Succeed \/ FailQuery \/ Cancel \/ CoordCrash \/ ClientObserve \/ ClientLost

\* The environment (crash, loss, cancel, wrong suspicion, coordinator crash) is never forced; everything else is.
Fairness ==
  /\ \A t \in Tasks, w \in Workers : WF_vars(Schedule(t, w))
  /\ WF_vars(\E m \in net : CoordAccept(m)) /\ WF_vars(\E m \in net : CoordReject(m))
  /\ WF_vars(\E m \in net : CoordAck(m))    /\ WF_vars(\E m \in net : WorkerStart(m))
  /\ WF_vars(\E m \in net : WorkerDiscard(m)) /\ WF_vars(\E m \in net : WorkerRelease(m))
  /\ \A w \in Workers : WF_vars(ReleaseSend(w)) /\ WF_vars(LeaseExpire(w))
  /\ \A w \in Workers : WF_vars(\E r \in running[w] : WorkerFinish(w, r))
  /\ \A w \in Workers : WF_vars(Suspect(w) /\ w \notin wUp)
  /\ \A t \in Tasks : WF_vars(Consume(t)) /\ WF_vars(ResendStart(t))
  /\ \A w \in Workers : WF_vars(\E r \in files[w] : WorkerResend(w, r))
  /\ WF_vars(Succeed) /\ WF_vars(FailQuery) /\ WF_vars(ClientObserve) /\ WF_vars(ClientLost)

Spec == Init /\ [][Next]_vars /\ Fairness

(* ---- properties ---- *)
TypeOK ==
  /\ qs \in {"running"} \cup Terminal /\ coordUp \in BOOLEAN /\ client \in {"waiting"} \cup Terminal
  /\ tstate \in [Tasks -> {"pending", "running", "done"}]
  /\ attempt \in [Tasks -> 0..(MaxAttempts + 1)]
  /\ assign \in [Tasks -> Workers \cup {NoWorker}] /\ commitW \in [Tasks -> Workers \cup {NoWorker}]
  /\ commit \in [Tasks -> 0..(MaxAttempts + 1)] /\ consumed \in [Tasks -> 0..(MaxAttempts + 1)]
  /\ dups \in Nat /\ suspect \subseteq Workers /\ wUp \subseteq Workers
  /\ released \subseteq Workers /\ cleaned \subseteq Workers

AllConsumedStrict      == \A t \in Tasks : consumed[t] /= 0
I1_NoDuplicateResult   == dups = 0
I2_SuccessIsComplete   == qs = "succeeded" => AllConsumedStrict
I3_ClientNeverWronglyTold == client = "succeeded" => qs = "succeeded"
I4_AttemptBound        == \A t \in Tasks : attempt[t] <= MaxAttempts
I5_CommitIsCurrent     == \A t \in Tasks : commit[t] /= 0 => (commit[t] = attempt[t] /\ commitW[t] = assign[t])
I6_ConsumedWasCommitted == \A t \in Tasks : consumed[t] /= 0 => commit[t] = consumed[t]

Frozen == (qs \in Terminal) => UNCHANGED <<commit, consumed>>
TerminalFrozen == [][Frozen]_vars
Clean == \A w \in wUp : files[w] = {} /\ running[w] = {}
L1_QueryEnds        == <>(QOver)
L2_NoOrphans        == QOver ~> Clean
L3_ClientLearns     == <>(client /= "waiting")

\* coverage canaries (each must be violated by TLC, proving the state is reachable): run with the cfg `canary-*`
C_Succeeds          == qs /= "succeeded"
C_Fails             == qs /= "failed"
C_Zombie            == ~(\E t \in Tasks : commit[t] /= 0 /\ commitW[t] \in suspect)
C_StaleRejected     == ~(\E m \in net : m.k = "discard")
C_CancelledMidRun   == qs /= "cancelled"
C_ReleasedWithFiles == ~(\E w \in released : w \in wUp /\ lost > 0)
=============================================================================
