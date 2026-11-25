--------------------------- MODULE SendProtocol ---------------------------
(***************************************************************************)
(* The duplicate-send protocol behind `jmap_send_draft` and                 *)
(* `jmap_reply_to_message` with send=true (src/guard.rs, src/tools.rs).     *)
(*                                                                         *)
(* A request is one intention: "send draft d" or "reply to m". It may make *)
(* several attempts (tool calls): a client that times out retries while    *)
(* the first call may still be running, and an agent may follow a failed   *)
(* reply's advice and send the draft it staged.                            *)
(*                                                                         *)
(* Concurrency. The server runs attempts on a multi-threaded runtime, so   *)
(* attempts really do run in parallel. A step below is one of:             *)
(*   - one acquisition of the guard's lock (begin, done, release, unknown, *)
(*     or a dropped permit): the only shared state, so every step that     *)
(*     reads or writes the guard is atomic;                                *)
(*   - one exchange with the JMAP server (a request on the wire, or its    *)
(*     answer).                                                            *)
(* Code between two such steps touches only the call's own locals, so      *)
(* its interleaving with other calls cannot matter.                        *)
(*                                                                         *)
(* The server is postmark2jmap's EmailSubmission/set with                  *)
(* onSuccessUpdateEmail. Worst case: it may refuse any submission, it      *)
(* delivers a message whether or not it is still a draft (it has no        *)
(* duplicate protection), the follow-up patch may fail, and any answer may *)
(* be lost. A request may sit on the wire and land after the client gave   *)
(* up on it. A call may also be cancelled at any point (its future is      *)
(* dropped).                                                               *)
(*                                                                         *)
(* Out of scope: guard entry expiry (10 minutes) and a process restart,    *)
(* both of which reopen the window by design.                              *)
(***************************************************************************)
EXTENDS Naturals, FiniteSets

CONSTANTS
    Drafts,       \* drafts already staged, the targets of send_draft requests
    Originals,    \* received messages, the targets of reply requests
    Fresh,        \* ids Email/set can mint for the drafts replies stage
    Requests,
    Target,       \* [Requests -> Drafts \cup Originals]
    Tries,        \* [Requests -> Nat]: calls per request, the first plus retries
    None

ASSUME Target \in [Requests -> Drafts \cup Originals]
ASSUME Tries \in [Requests -> Nat \ {0}]

Emails == Drafts \cup Originals \cup Fresh
Attempts == UNION {{r} \X (1 .. Tries[r]) : r \in Requests}
Req(a) == a[1]
Prev(a) == <<a[1], a[2] - 1>>

\* The guard is keyed by tool and a retry-stable id.
Subjects == ({"send_draft"} \X (Drafts \cup Fresh)) \cup ({"reply"} \X Originals)

\* Tool results, by what they tell the agent.
SentResults == {"sent", "sent_unfiled"}
NothingSentResults ==                       \* each says: this call sent nothing
    {"replayed", "not_found", "already_sent", "refused_in_flight",
     "refused_unknown", "rejected", "reply_rejected"}

VARIABLES
    email,      \* [Emails -> {"absent", "draft", "sent", "received"}]
    guard,      \* [Subjects -> {"free", "in_flight", "unknown", "done"}]
    pc,         \* [Attempts -> step]
    tool,       \* [Attempts -> tool the attempt calls]
    draft,      \* [Attempts -> the Email the attempt submits, or None]
    held,       \* [Attempts -> SUBSET Subjects]: subjects the attempt reserved
    net,        \* submissions on the wire, not yet processed by the server
    answer,     \* [Attempts -> server answer to the attempt's submission]
    result,     \* [Attempts -> the tool result]
    advice,     \* [Attempts -> draft the result says is safe to send, or None]
    delivered,  \* [Emails -> Nat]: copies handed to SMTP
    sentBy      \* [Attempts -> BOOLEAN]: the attempt's submission was delivered

vars == <<email, guard, pc, tool, draft, held, net, answer, result, advice,
          delivered, sentBy>>

TypeOK ==
    /\ email \in [Emails -> {"absent", "draft", "sent", "received"}]
    /\ guard \in [Subjects -> {"free", "in_flight", "unknown", "done"}]
    /\ held \in [Attempts -> SUBSET Subjects]
    /\ answer \in [Attempts -> {"none", "rejected", "filed", "unfiled"}]
    /\ delivered \in [Emails -> Nat]
    /\ sentBy \in [Attempts -> BOOLEAN]

Init ==
    /\ email = [e \in Emails |-> IF e \in Drafts THEN "draft"
                                 ELSE IF e \in Originals THEN "received"
                                 ELSE "absent"]
    /\ guard = [s \in Subjects |-> "free"]
    /\ pc = [a \in Attempts |-> "idle"]
    /\ tool = [a \in Attempts |-> "none"]
    /\ draft = [a \in Attempts |-> None]
    /\ held = [a \in Attempts |-> {}]
    /\ net = {}
    /\ answer = [a \in Attempts |-> "none"]
    /\ result = [a \in Attempts |-> "none"]
    /\ advice = [a \in Attempts |-> None]
    /\ delivered = [e \in Emails |-> 0]
    /\ sentBy = [a \in Attempts |-> FALSE]

-----------------------------------------------------------------------------
(* The guard (Guard::begin and Permit).                                    *)

\* Settle a's reservations: free them (a definite "no"), mark them done (the
\* message was accepted), or leave them unknown (it may have gone out).
Settle(a, to) == [s \in Subjects |-> IF s \in held[a] THEN to ELSE guard[s]]

Finish(a, res) ==
    /\ pc' = [pc EXCEPT ![a] = "done"]
    /\ result' = [result EXCEPT ![a] = res]

Running(a) == pc[a] \notin {"idle", "done"}

Refusal(s) == IF guard[s] = "in_flight" THEN "refused_in_flight" ELSE "refused_unknown"

-----------------------------------------------------------------------------
(* The tools. Each starts with one atomic step, before its first await:    *)
(* the replay lookup and the reservation, under the same lock.             *)

Start(a, t, d, s) ==
    /\ tool' = [tool EXCEPT ![a] = t]
    /\ draft' = [draft EXCEPT ![a] = d]
    /\ IF guard[s] = "done"
       THEN /\ Finish(a, "replayed")
            /\ UNCHANGED <<guard, held>>
       ELSE IF guard[s] # "free"
       THEN /\ Finish(a, Refusal(s))
            /\ UNCHANGED <<guard, held>>
       ELSE /\ held' = [held EXCEPT ![a] = {s}]
            /\ guard' = [guard EXCEPT ![s] = "in_flight"]
            /\ pc' = [pc EXCEPT ![a] = IF t = "send_draft" THEN "load" ELSE "stage"]
            /\ UNCHANGED result

StartSendDraft(a, d) == Start(a, "send_draft", d, <<"send_draft", d>>)

StartAttempt(a) ==
    /\ pc[a] = "idle"
    /\ \/ a[2] = 1
       \/ a[2] > 1 /\ pc[Prev(a)] # "idle"
    /\ LET r == Req(a) IN
       \/ Target[r] \in Drafts /\ StartSendDraft(a, Target[r])
       \/ /\ Target[r] \in Originals
          /\ \A b \in Attempts : (Req(b) = r /\ b[2] < a[2]) => tool[b] # "send_draft"
          /\ Start(a, "reply", None, <<"reply", Target[r]>>)
       \* An agent acting on the latest answer: send the draft a rejected reply
       \* named as safe to send, or retry that send_draft.
       \/ /\ Target[r] \in Originals
          /\ a[2] > 1
          /\ \E x \in {advice[Prev(a)]} \cup
                      (IF tool[Prev(a)] = "send_draft" THEN {draft[Prev(a)]} ELSE {}) :
                /\ x # None
                /\ StartSendDraft(a, x)
    /\ UNCHANGED <<email, net, answer, advice, delivered, sentBy>>

\* send_draft: fetch_draft reads the draft inside the reservation. Anything
\* that is not a draft is a definite "no".
Load(a) ==
    /\ pc[a] = "load"
    /\ CASE email[draft[a]] = "absent" ->
              guard' = Settle(a, "free") /\ Finish(a, "not_found")
         [] email[draft[a]] = "draft" ->
              pc' = [pc EXCEPT ![a] = "submit"] /\ UNCHANGED <<guard, result>>
         [] OTHER ->
              guard' = Settle(a, "free") /\ Finish(a, "already_sent")
    /\ UNCHANGED <<email, tool, draft, held, net, answer, advice, delivered, sentBy>>

\* reply with send=true: Email/set creates the reply as a draft, then the inner
\* send_draft reserves it under its own subject.
Stage(a) ==
    /\ pc[a] = "stage"
    /\ \E f \in Fresh :
         /\ email[f] = "absent"
         /\ \A b \in Attempts : draft[b] # f
         /\ email' = [email EXCEPT ![f] = "draft"]
         /\ draft' = [draft EXCEPT ![a] = f]
         /\ LET s == <<"send_draft", f>> IN
            IF guard[s] # "free"
            THEN /\ guard' = Settle(a, "free")
                 /\ Finish(a, Refusal(s))
                 /\ UNCHANGED held
            ELSE /\ held' = [held EXCEPT ![a] = @ \cup {s}]
                 /\ guard' = [guard EXCEPT ![s] = "in_flight"]
                 /\ pc' = [pc EXCEPT ![a] = "load"]
                 /\ UNCHANGED result
    /\ UNCHANGED <<tool, net, answer, advice, delivered, sentBy>>

\* submit hands the submission to the wire.
Submit(a) ==
    /\ pc[a] = "submit"
    /\ net' = net \cup {[att |-> a, email |-> draft[a]]}
    /\ pc' = [pc EXCEPT ![a] = "wait"]
    /\ UNCHANGED <<email, guard, tool, draft, held, answer, result, advice,
                   delivered, sentBy>>

\* The answer arrives. Recording the result and ending the reservation are one
\* lock acquisition (Permit::done), so a retry that gets the reservation always
\* finds the result.
Receive(a) ==
    /\ pc[a] = "wait"
    /\ answer[a] # "none"
    /\ CASE answer[a] = "rejected" ->
              \* A JMAP error type: the server said no. Nothing was sent.
              /\ guard' = Settle(a, "free")
              /\ Finish(a, IF tool[a] = "reply" THEN "reply_rejected" ELSE "rejected")
              /\ advice' = IF tool[a] = "reply" THEN [advice EXCEPT ![a] = draft[a]] ELSE advice
         [] answer[a] = "filed" ->
              /\ guard' = Settle(a, "done")
              /\ Finish(a, "sent")
              /\ UNCHANGED advice
         [] answer[a] = "unfiled" ->
              \* Delivered but the follow-up patch failed: still a result, with a warning.
              /\ guard' = Settle(a, "done")
              /\ Finish(a, "sent_unfiled")
              /\ UNCHANGED advice
    /\ UNCHANGED <<email, tool, draft, held, net, answer, delivered, sentBy>>

\* Timeout, reset, a 5xx, or an answer lost on the way back: delivery unknown.
\* The request itself may still be on the wire.
Lose(a) ==
    /\ pc[a] = "wait"
    /\ guard' = Settle(a, "unknown")
    /\ Finish(a, "unknown")
    /\ UNCHANGED <<email, tool, draft, held, net, answer, advice, delivered, sentBy>>

\* The call's future is dropped at any point (a cancelled request): the permit
\* drops unsettled, which reads as unknown.
Cancel(a) ==
    /\ Running(a)
    /\ guard' = Settle(a, "unknown")
    /\ Finish(a, "cancelled")
    /\ UNCHANGED <<email, tool, draft, held, net, answer, advice, delivered, sentBy>>

-----------------------------------------------------------------------------
(* The server.                                                             *)

Process(m) ==
    /\ net' = net \ {m}
    /\ \/ /\ answer' = [answer EXCEPT ![m.att] = "rejected"]
          /\ UNCHANGED <<email, delivered, sentBy>>
       \/ /\ delivered' = [delivered EXCEPT ![m.email] = @ + 1]
          /\ sentBy' = [sentBy EXCEPT ![m.att] = TRUE]
          /\ \/ /\ email' = [email EXCEPT ![m.email] = "sent"]
                /\ answer' = [answer EXCEPT ![m.att] = "filed"]
             \/ /\ UNCHANGED email
                /\ answer' = [answer EXCEPT ![m.att] = "unfiled"]
    /\ UNCHANGED <<guard, pc, tool, draft, held, result, advice>>

-----------------------------------------------------------------------------

Step(a) == Load(a) \/ Stage(a) \/ Submit(a) \/ Receive(a) \/ Lose(a) \/ Cancel(a)

\* Everything that started has returned and the wire is empty.
Quiescent == (\A a \in Attempts : ~Running(a)) /\ net = {}

Next ==
    \/ \E a \in Attempts : StartAttempt(a) \/ Step(a)
    \/ \E m \in net : Process(m)
    \/ Quiescent /\ UNCHANGED vars

\* A call keeps making progress, and the server eventually answers what
\* reaches it. Attempts need not start: a client may stop retrying.
Fairness ==
    /\ \A a \in Attempts : WF_vars(Step(a))
    /\ \A m \in [att : Attempts, email : Emails] : WF_vars(m \in net /\ Process(m))

Spec == Init /\ [][Next]_vars /\ Fairness

-----------------------------------------------------------------------------
(* Properties.                                                             *)

Deliveries(r) == Cardinality({a \in Attempts : Req(a) = r /\ sentBy[a]})

\* One intention, at most one message out, however it was retried.
AtMostOncePerRequest == \A r \in Requests : Deliveries(r) <= 1

\* No Email is handed to SMTP twice, across all requests.
AtMostOncePerEmail == \A e \in Emails : delivered[e] <= 1

\* A result that says nothing was sent is true, even after a request still on
\* the wire lands. And "sent" is only said of a delivery.
TruthfulResults ==
    \A a \in Attempts :
        /\ result[a] \in NothingSentResults => ~sentBy[a]
        /\ result[a] \in SentResults => sentBy[a]

\* A subject is only in flight while a running call holds it: no path leaks
\* a reservation.
NoLeakedReservation ==
    \A s \in Subjects :
        guard[s] = "in_flight" => \E a \in Attempts : Running(a) /\ s \in held[a]

\* A recorded result is only recorded for something that was delivered.
DoneMeansDelivered ==
    \A s \in Subjects :
        guard[s] = "done" => \E a \in Attempts : s \in held[a] /\ sentBy[a]

\* Every call that starts returns.
CallsReturn == \A a \in Attempts : (pc[a] # "idle") ~> (pc[a] = "done")

\* Non-vacuity: TLC is run once with this as an invariant and must find it
\* violated, proving a message can be sent at all in the model.
NothingEverSent == \A e \in Emails : delivered[e] = 0

=============================================================================
