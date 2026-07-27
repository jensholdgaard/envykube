// Deterministic admission fault injector for the envykube chaos suite.
//
// Two listeners:
//   :8443 (TLS)   /validate                    — the ValidatingAdmissionWebhook endpoint
//   :8080 (plain) /arm /reset /state /healthz  — control API, reached through the vCluster
//                                                API server's service proxy, so neither the
//                                                runner nor the agent needs port-forward or
//                                                an Ingress to drive it.
//
// The fault is an ORDINAL, never a rate: "fail the Nth matching write". Same scenario, same
// arming => the same call fails every time, so a failure is reproducible and a fix is
// provably a fix. A percentage would hand a weak model a different failure each attempt,
// which is precisely the feedback that cannot be iterated against.
//
// Loaded from a ConfigMap (see templates/webhook.yaml): agents have no Docker socket and the
// platform has no in-cluster registry, so a component requiring an image build could not be
// installed from inside a vCluster at all.

const TLS_KEY = process.env.TLS_KEY || "/tls/tls.key";
const TLS_CRT = process.env.TLS_CRT || "/tls/tls.crt";
const CONTROL_PORT = Number(process.env.CONTROL_PORT || 8080);
const WEBHOOK_PORT = Number(process.env.WEBHOOK_PORT || 8443);

const MODES = {
  // Names chosen for what they teach, not for the HTTP code:
  //   conflict → does the caller re-read and merge, or blindly clobber?
  //   error    → does the caller back off and retry, or abort the whole deploy?
  //   throttle → does the caller honour backpressure?
  conflict: { code: 409, reason: "Conflict", message: "chaos: simulated write conflict" },
  error: { code: 500, reason: "InternalError", message: "chaos: simulated server error" },
  throttle: { code: 429, reason: "TooManyRequests", message: "chaos: simulated throttling" },
};

let armed = null;
let seen = 0;
let fired = [];

function json(body, status) {
  return new Response(JSON.stringify(body), {
    status: status || 200,
    headers: { "content-type": "application/json" },
  });
}

function allow(uid) {
  const response = { uid: uid, allowed: true };
  return { apiVersion: "admission.k8s.io/v1", kind: "AdmissionReview", response: response };
}

function deny(uid, mode) {
  const response = {
    uid: uid,
    allowed: false,
    status: { code: mode.code, reason: mode.reason, message: mode.message },
  };
  return { apiVersion: "admission.k8s.io/v1", kind: "AdmissionReview", response: response };
}

function nameOf(req) {
  if (req.name) return req.name;
  const obj = req.object || {};
  const meta = obj.metadata || {};
  return meta.name || meta.generateName || "";
}

function matches(req) {
  const anyOf = (list, value) => !list || list.length === 0 || list.indexOf(value) !== -1;
  const resource = (req.resource || {}).resource;
  if (!anyOf(armed.namespaces, req.namespace)) return false;
  if (!anyOf(armed.resources, resource)) return false;
  if (!anyOf(armed.operations, req.operation)) return false;
  if (armed.names && armed.names.length > 0) {
    const name = nameOf(req);
    if (!armed.names.some((prefix) => name.indexOf(prefix) === 0)) return false;
  }
  return true;
}

async function validate(request) {
  let body;
  try {
    body = await request.json();
  } catch (_) {
    return json({ error: "bad admission payload" }, 400);
  }
  const req = body.request || {};
  const uid = req.uid;

  if (!armed) return json(allow(uid));
  if (!matches(req)) return json(allow(uid));

  seen += 1;
  if (armed.ordinals.indexOf(seen) === -1) return json(allow(uid));

  const mode = MODES[armed.mode] || MODES.error;
  fired.push({
    ordinal: seen,
    operation: req.operation,
    resource: (req.resource || {}).resource,
    namespace: req.namespace,
    name: nameOf(req),
    code: mode.code,
  });
  console.log(
    `[fault] denied #${seen} ${req.operation} ${req.namespace}/${nameOf(req)} -> ${mode.code}`,
  );
  return json(deny(uid, mode));
}

function stateBody() {
  return { armed: armed, seen: seen, fired: fired, firedCount: fired.length };
}

async function control(request) {
  const url = new URL(request.url);

  if (url.pathname === "/healthz") return new Response("ok");
  if (url.pathname === "/state") return json(stateBody());

  if (url.pathname === "/reset" && request.method === "POST") {
    armed = null;
    seen = 0;
    fired = [];
    console.log("[fault] disarmed");
    return json(stateBody());
  }

  if (url.pathname === "/arm" && request.method === "POST") {
    let spec;
    try {
      spec = await request.json();
    } catch (_) {
      return json({ error: "body must be JSON" }, 400);
    }
    if (!MODES[spec.mode]) {
      return json({ error: `mode must be one of ${Object.keys(MODES).join(", ")}` }, 400);
    }
    const ordinals = (spec.ordinals || [1]).map(Number).filter((n) => n > 0);
    if (ordinals.length === 0) return json({ error: "ordinals must be positive integers" }, 400);
    armed = {
      mode: spec.mode,
      ordinals: ordinals,
      namespaces: spec.namespaces || [],
      resources: spec.resources || [],
      operations: spec.operations || [],
      names: spec.names || [],
    };
    seen = 0;
    fired = [];
    console.log(`[fault] armed ${armed.mode} on ordinals ${ordinals.join(",")}`);
    return json(stateBody());
  }

  return json({ error: "not found" }, 404);
}

Bun.serve({
  port: WEBHOOK_PORT,
  tls: { key: Bun.file(TLS_KEY), cert: Bun.file(TLS_CRT) },
  fetch(request) {
    const url = new URL(request.url);
    if (url.pathname === "/validate") return validate(request);
    if (url.pathname === "/healthz") return new Response("ok");
    return new Response("not found", { status: 404 });
  },
});

Bun.serve({ port: CONTROL_PORT, fetch: control });

console.log(`fault-webhook listening: admission :${WEBHOOK_PORT} (tls), control :${CONTROL_PORT}`);
