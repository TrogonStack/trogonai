(() => {
  "use strict";

  const STREAM_LEASES = "sandbox.leases";
  const STREAM_SNAPSHOTS = "sandbox.snapshots";
  const STREAM_WORKERS = "sandbox.workers";

  const leasesById = new Map();
  const snapshotsByDigest = new Map();
  const workersByKey = new Map();
  const tagsByKey = new Map();

  let ws = null;
  let requestSeq = 0;
  const pending = new Map();

  const connectForm = document.getElementById("connect-form");
  const wsUrlInput = document.getElementById("ws-url");
  const tokenInput = document.getElementById("bearer-token");
  const connectBtn = document.getElementById("connect-btn");
  const statusEl = document.getElementById("connection-status");
  const dashboardEl = document.getElementById("dashboard");

  const leasesBody = document.getElementById("leases-body");
  const snapshotsBody = document.getElementById("snapshots-body");
  const workersBody = document.getElementById("workers-body");
  const tagsBody = document.getElementById("tags-body");

  function nextId(prefix) {
    requestSeq += 1;
    return `${prefix}-${requestSeq}`;
  }

  function setStatus(text, kind) {
    statusEl.textContent = text;
    statusEl.className = `status${kind ? ` ${kind}` : ""}`;
  }

  function readQueryParams() {
    const params = new URLSearchParams(window.location.search);
    const wsGateway = params.get("ws_gateway") || params.get("ws");
    const token = params.get("token") || params.get("bearer");
    if (wsGateway) {
      wsUrlInput.value = wsGateway;
    }
    if (token) {
      tokenInput.value = token;
    }
  }

  async function loadDefaults() {
    try {
      const resp = await fetch("/config.json");
      if (!resp.ok) {
        return;
      }
      const cfg = await resp.json();
      if (cfg.ws_gateway_url && !wsUrlInput.value) {
        wsUrlInput.value = cfg.ws_gateway_url;
      }
    } catch (_err) {
      // config.json is optional during local file preview
    }
  }

  function formatEpoch(secs) {
    if (secs == null || secs === 0) {
      return "-";
    }
    return new Date(secs * 1000).toISOString();
  }

  function formatImage(image) {
    if (!image || typeof image !== "object") {
      return "-";
    }
    switch (image.kind) {
      case "registry": {
        const ref = image.reference || {};
        const tag = ref.tag ? `:${ref.tag}` : "";
        const digest = ref.digest ? `@${ref.digest}` : "";
        const name = ref.registry
          ? `${ref.registry}/${ref.repository}`
          : ref.repository || "-";
        return `${name}${tag}${digest}`;
      }
      case "snapshot": {
        const ref = image.reference || {};
        return ref.digest || ref.repository || "-";
      }
      case "snapshot_tag": {
        const tag = image.tag || {};
        return `tag:${tag.tenant || "?"}:${tag.name || "?"}`;
      }
      case "hyperlight_module": {
        const ref = image.reference || {};
        return ref.digest || ref.repository || "-";
      }
      default:
        return JSON.stringify(image);
    }
  }

  function formatProvider(provider) {
    if (provider == null) {
      return "-";
    }
    if (typeof provider === "string") {
      return provider;
    }
    return provider.toString();
  }

  function formatArtifactType(value) {
    if (value == null) {
      return "-";
    }
    if (typeof value === "string") {
      return value;
    }
    if (typeof value === "object") {
      return Object.keys(value)[0] || JSON.stringify(value);
    }
    return String(value);
  }

  function profileSummary(summary) {
    if (!summary) {
      return "-";
    }
    const parts = [];
    if (summary.can_snapshot) parts.push("snapshot");
    if (summary.can_restore) parts.push("restore");
    if (summary.can_checkpoint_running) parts.push("checkpoint");
    if (summary.supports_warm_pool) parts.push("warm_pool");
    if (summary.supports_named_tags) parts.push("named_tags");
    const tiers = (summary.isolation_tiers || []).join(",");
    if (tiers) parts.push(`tiers:${tiers}`);
    return parts.length ? parts.join(", ") : "default";
  }

  function renderEmptyRow(tbody, colSpan, message) {
    tbody.innerHTML = `<tr class="empty-row"><td colspan="${colSpan}">${message}</td></tr>`;
  }

  function renderLeases() {
    const rows = [...leasesById.values()].sort((a, b) =>
      (a.lease_id || "").localeCompare(b.lease_id || "")
    );
    if (!rows.length) {
      renderEmptyRow(leasesBody, 6, "No active leases");
      return;
    }
    leasesBody.innerHTML = rows
      .map(
        (row) => `<tr>
          <td>${escapeHtml(row.tenant || "-")}</td>
          <td>${escapeHtml(formatProvider(row.provider))}</td>
          <td class="mono">${escapeHtml(formatImage(row.image))}</td>
          <td>${escapeHtml(row.state || "-")}</td>
          <td>${escapeHtml(formatEpoch(row.started_at))}</td>
          <td>${escapeHtml(String(row.ttl_seconds ?? "-"))}</td>
        </tr>`
      )
      .join("");
  }

  function renderSnapshots() {
    const rows = [...snapshotsByDigest.values()].sort((a, b) =>
      (a.digest || "").localeCompare(b.digest || "")
    );
    if (!rows.length) {
      renderEmptyRow(snapshotsBody, 5, "No snapshots");
      return;
    }
    snapshotsBody.innerHTML = rows
      .map(
        (row) => `<tr>
          <td class="mono">${escapeHtml(row.digest || "-")}</td>
          <td>${escapeHtml(formatArtifactType(row.artifact_type))}</td>
          <td>${escapeHtml(row.tenant || "-")}</td>
          <td>${escapeHtml(formatEpoch(row.published_at))}</td>
          <td>${escapeHtml(row.bytes == null ? "-" : String(row.bytes))}</td>
        </tr>`
      )
      .join("");
  }

  function renderWorkers() {
    const rows = [...workersByKey.values()].sort((a, b) =>
      `${a.provider}:${a.worker_id}`.localeCompare(`${b.provider}:${b.worker_id}`)
    );
    if (!rows.length) {
      renderEmptyRow(workersBody, 5, "No worker heartbeats yet");
      return;
    }
    workersBody.innerHTML = rows
      .map(
        (row) => `<tr>
          <td>${escapeHtml(formatProvider(row.provider))}</td>
          <td class="mono">${escapeHtml(row.worker_id || "-")}</td>
          <td>${escapeHtml(profileSummary(row.capability_summary))}</td>
          <td>${escapeHtml(String(row.cached_digests_count ?? 0))}</td>
          <td>${escapeHtml(formatEpoch(row.last_heartbeat))}</td>
        </tr>`
      )
      .join("");
  }

  function renderTags() {
    const rows = [...tagsByKey.values()].sort((a, b) =>
      `${a.tenant}:${a.name}`.localeCompare(`${b.tenant}:${b.name}`)
    );
    if (!rows.length) {
      renderEmptyRow(tagsBody, 4, "No snapshot tags");
      return;
    }
    tagsBody.innerHTML = rows
      .map(
        (row) => `<tr>
          <td>${escapeHtml(row.tenant || "-")}</td>
          <td>${escapeHtml(row.name || "-")}</td>
          <td class="mono">${escapeHtml(row.digest || "-")}</td>
          <td>${escapeHtml(formatArtifactType(row.artifact_type))}</td>
        </tr>`
      )
      .join("");
  }

  function escapeHtml(value) {
    return String(value)
      .replaceAll("&", "&amp;")
      .replaceAll("<", "&lt;")
      .replaceAll(">", "&gt;")
      .replaceAll('"', "&quot;");
  }

  function sendRequest(tool, payload = {}) {
    return new Promise((resolve, reject) => {
      if (!ws || ws.readyState !== WebSocket.OPEN) {
        reject(new Error("websocket not connected"));
        return;
      }
      const id = nextId(tool);
      pending.set(id, { resolve, reject });
      ws.send(
        JSON.stringify({
          kind: "request",
          id,
          tool,
          payload,
        })
      );
    });
  }

  function handleResponse(frame) {
    const waiter = pending.get(frame.id);
    if (!waiter) {
      return;
    }
    pending.delete(frame.id);
    if (frame.error) {
      waiter.reject(new Error(frame.error.message || frame.error.code || "request failed"));
      return;
    }
    waiter.resolve(frame.result);
  }

  function upsertLease(row) {
    if (!row || !row.lease_id) {
      return;
    }
    leasesById.set(row.lease_id, row);
    renderLeases();
  }

  function removeLease(leaseId) {
    leasesById.delete(leaseId);
    renderLeases();
  }

  function upsertSnapshot(row) {
    if (!row || !row.digest) {
      return;
    }
    const existing = snapshotsByDigest.get(row.digest) || {};
    snapshotsByDigest.set(row.digest, { ...existing, ...row });
    renderSnapshots();
  }

  function upsertWorker(row) {
    if (!row || !row.worker_id) {
      return;
    }
    const key = `${formatProvider(row.provider)}:${row.worker_id}`;
    workersByKey.set(key, row);
    renderWorkers();
  }

  function upsertTag(row) {
    if (!row || !row.name) {
      return;
    }
    const key = `${row.tenant}:${row.name}`;
    tagsByKey.set(key, row);
    renderTags();
  }

  function handleEvent(frame) {
    const streamId = frame.stream_id;
    const event = frame.event || {};

    if (streamId === STREAM_LEASES) {
      if (event.action === "delete" && event.lease && event.lease.lease_id) {
        removeLease(event.lease.lease_id);
        return;
      }
      if (event.lease) {
        upsertLease(event.lease);
      }
      return;
    }

    if (streamId === STREAM_SNAPSHOTS) {
      upsertSnapshot(event);
      return;
    }

    if (streamId === STREAM_WORKERS) {
      upsertWorker(event);
    }
  }

  async function bootstrapData() {
    const [leasesResult, snapshotsResult, tagsResult] = await Promise.all([
      sendRequest("sandbox_list_leases"),
      sendRequest("sandbox_list_snapshots"),
      sendRequest("sandbox_list_tags"),
    ]);

    leasesById.clear();
    for (const row of leasesResult.leases || []) {
      upsertLease(row);
    }

    snapshotsByDigest.clear();
    for (const row of snapshotsResult.snapshots || []) {
      upsertSnapshot(row);
    }

    tagsByKey.clear();
    for (const row of tagsResult.tags || []) {
      upsertTag(row);
    }

    await sendRequest("sandbox_subscribe_dashboard");
  }

  function connectWebSocket(url, token) {
    if (ws) {
      ws.close();
      ws = null;
    }

    setStatus("connecting");
    connectBtn.disabled = true;

    ws = new WebSocket(url, [`bearer.${token}`]);

    ws.onopen = async () => {
      setStatus("connected", "connected");
      dashboardEl.classList.remove("hidden");
      connectBtn.disabled = false;
      try {
        await bootstrapData();
      } catch (err) {
        setStatus(`connected (bootstrap failed: ${err.message})`, "error");
      }
    };

    ws.onmessage = (ev) => {
      let frame;
      try {
        frame = JSON.parse(ev.data);
      } catch (_err) {
        return;
      }
      if (frame.kind === "response") {
        handleResponse(frame);
      } else if (frame.kind === "event") {
        handleEvent(frame);
      }
    };

    ws.onerror = () => {
      setStatus("websocket error", "error");
      connectBtn.disabled = false;
    };

    ws.onclose = () => {
      if (statusEl.textContent === "connected") {
        setStatus("disconnected", "error");
      }
      connectBtn.disabled = false;
    };
  }

  connectForm.addEventListener("submit", (ev) => {
    ev.preventDefault();
    const url = wsUrlInput.value.trim();
    const token = tokenInput.value.trim();
    if (!url || !token) {
      setStatus("ws url and bearer token required", "error");
      return;
    }
    connectWebSocket(url, token);
  });

  readQueryParams();
  loadDefaults();
})();
