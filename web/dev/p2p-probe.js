/* 直连探测页的脚本（p2p-probe.html）。测量工具，不是产品功能。
 *
 * 流程：从网址 # 取房间号 → 连 hub 的信令房间（/v1/probe/rooms/<room>?role=page，
 * 只在 hub 开了 DROPPEN_HUB_P2P_PROBE=1 时存在）→ 和电脑上的 rh-p2p-probe 交换
 * offer/answer/候选（trickle ICE，STUN 用 Cloudflare 和 Google，不用 TURN）→
 * 直连 DataChannel 和 hub 路径各 ping 200 次（交替进行，网络起伏对两边公平）→
 * 把结果发回探针（优先走 DataChannel，不通就走房间），探针写进 results.jsonl。
 *
 * 网址参数（自动化用）：?label=same-machine&auto=1 选好网络并立即开始；
 * &nodirect=1 让浏览器只许用中继候选（又没有 TURN），直连必然失败，用来测失败路径。
 */
"use strict";

(function () {
  var STUN = [
    { urls: "stun:stun.cloudflare.com:3478", name: "cloudflare" },
    { urls: "stun:stun.l.google.com:19302", name: "google" },
  ];
  var PINGS = 200;
  var CONNECT_TIMEOUT_MS = 15000;
  var DC_PING_TIMEOUT_MS = 3000;
  var HUB_PING_TIMEOUT_MS = 5000;
  var LABELS = [
    ["same-machine", "同一台机器"],
    ["home-wifi", "家里 Wi-Fi"],
    ["cellular", "手机流量"],
    ["office", "公司网络"],
    ["other", "其他"],
  ];
  var VERSION = 1;

  function $(id) {
    return document.getElementById(id);
  }
  function set(id, text) {
    $(id).textContent = text;
  }
  function status(text) {
    set("status", text);
  }
  function now() {
    return performance.now();
  }
  function round1(x) {
    return x == null ? null : Math.round(x * 10) / 10;
  }
  function sleep(ms) {
    return new Promise(function (r) {
      setTimeout(r, ms);
    });
  }
  function randomHex(bytes) {
    var a = new Uint8Array(bytes);
    crypto.getRandomValues(a);
    return Array.prototype.map
      .call(a, function (b) {
        return (b < 16 ? "0" : "") + b.toString(16);
      })
      .join("");
  }
  function withTimeout(promise, ms, value) {
    return Promise.race([
      promise,
      new Promise(function (r) {
        setTimeout(function () {
          r(value);
        }, ms);
      }),
    ]);
  }

  /* p50/p95/max over the answered pings; lost = unanswered. */
  function summarize(rtts) {
    var ok = rtts.filter(function (x) {
      return x != null;
    });
    ok.sort(function (a, b) {
      return a - b;
    });
    function pct(p) {
      if (!ok.length) return null;
      return round1(ok[Math.min(ok.length - 1, Math.max(0, Math.ceil((p / 100) * ok.length) - 1))]);
    }
    return {
      sent: rtts.length,
      ok: ok.length,
      lost: rtts.length - ok.length,
      min: ok.length ? round1(ok[0]) : null,
      p50: pct(50),
      p95: pct(95),
      max: ok.length ? round1(ok[ok.length - 1]) : null,
    };
  }

  function showPings(prefix, rtts) {
    var s = summarize(rtts);
    set(prefix + "-n", s.sent + "/" + PINGS);
    set(prefix + "-p50", s.p50 == null ? "—" : s.p50.toFixed(1));
    set(prefix + "-p95", s.p95 == null ? "—" : s.p95.toFixed(1));
    set(prefix + "-max", s.max == null ? "—" : s.max.toFixed(1));
    set(prefix + "-lost", String(s.lost));
    return s;
  }

  /* A ping that is answered by seq, or resolves to null after a timeout. */
  function Pinger(send, timeoutMs) {
    var pending = {};
    var seq = 0;
    this.ping = function () {
      var s = ++seq;
      return new Promise(function (resolve) {
        var t0 = now();
        var timer = setTimeout(function () {
          delete pending[s];
          resolve(null);
        }, timeoutMs);
        pending[s] = function (t1) {
          clearTimeout(timer);
          resolve(t1 - t0);
        };
        try {
          send(s);
        } catch (e) {
          clearTimeout(timer);
          delete pending[s];
          resolve(null);
        }
      });
    };
    this.pong = function (s, t1) {
      var done = pending[s];
      if (done) {
        delete pending[s];
        done(t1);
      }
    };
  }

  /* The hub's signaling room. */
  function Room(roomId, sid) {
    var self = this;
    var proto = location.protocol === "https:" ? "wss:" : "ws:";
    this.url = proto + "//" + location.host + "/v1/probe/rooms/" + roomId + "?role=page";
    this.sid = sid;
    this.probes = 0;
    this.handlers = {};
    this.onPeers = function () {};
    var peersSeen;
    this.peersSeen = new Promise(function (r) {
      peersSeen = r;
    });
    this.open = function () {
      return new Promise(function (resolve, reject) {
        var ws = new WebSocket(self.url);
        var opened = false;
        self.ws = ws;
        var t0 = now();
        ws.onopen = function () {
          opened = true;
          resolve(now() - t0);
        };
        ws.onerror = function () {
          if (!opened) reject(new Error("连不上 hub 房间（hub 没开 DROPPEN_HUB_P2P_PROBE，或被限流）"));
        };
        ws.onclose = function (ev) {
          if (!opened) reject(new Error("hub 房间关闭了：" + ev.code + " " + (ev.reason || "")));
          self.closed = ev.code + " " + (ev.reason || "");
        };
        ws.onmessage = function (ev) {
          var t1 = now();
          var m;
          try {
            m = JSON.parse(ev.data);
          } catch (e) {
            return;
          }
          if (m.hub === "peers") {
            self.probes = m.probes || 0;
            self.onPeers(m);
            peersSeen();
            return;
          }
          if (m.s !== self.sid) return;
          var h = self.handlers[m.t];
          if (h) h(m, t1);
        };
      });
    };
    this.send = function (obj) {
      if (!self.ws || self.ws.readyState !== 1) return false;
      obj.s = self.sid;
      self.ws.send(JSON.stringify(obj));
      return true;
    };
    this.waitFor = function (type, ms) {
      return withTimeout(
        new Promise(function (resolve) {
          var prev = self.handlers[type];
          self.handlers[type] = function (m, t1) {
            self.handlers[type] = prev;
            if (prev) prev(m, t1);
            resolve(m);
          };
        }),
        ms,
        null,
      );
    };
    this.close = function () {
      try {
        self.ws.close();
      } catch (e) {
        /* already closed */
      }
    };
  }

  /* An a=candidate line's parts (works on mDNS host names too). */
  function parseCandidate(line) {
    var f = (line || "").replace(/^a=/, "").split(/\s+/);
    var typ = f.indexOf("typ");
    return {
      protocol: (f[2] || "").toLowerCase(),
      address: f[4] || "",
      port: Number(f[5]) || 0,
      type: typ >= 0 ? f[typ + 1] : "",
    };
  }

  function family(address) {
    if (!address) return "";
    if (/\.local$/.test(address)) return "mDNS";
    return address.indexOf(":") >= 0 ? "IPv6" : "IPv4";
  }

  /* The selected candidate pair, from getStats (Chrome: transport stats;
   * Safari/Firefox: the nominated, succeeded or selected pair). */
  function selectedPair(pc) {
    return pc.getStats().then(function (stats) {
      var pair = null;
      stats.forEach(function (s) {
        if (s.type === "transport" && s.selectedCandidatePairId) pair = stats.get(s.selectedCandidatePairId) || pair;
      });
      if (!pair) {
        stats.forEach(function (s) {
          if (s.type === "candidate-pair" && (s.selected || (s.nominated && s.state === "succeeded")) && !pair) pair = s;
        });
      }
      if (!pair) return null;
      function cand(id) {
        var c = stats.get(id);
        if (!c) return null;
        var address = c.address || c.ip || "";
        return {
          type: c.candidateType,
          protocol: c.protocol,
          address: address,
          family: family(address),
          port: c.port,
          network: c.networkType || null,
        };
      }
      return {
        local: cand(pair.localCandidateId),
        remote: cand(pair.remoteCandidateId),
        stun_rtt_ms: pair.currentRoundTripTime != null ? round1(pair.currentRoundTripTime * 1000) : null,
        state: pair.state,
      };
    });
  }

  function pairText(p) {
    if (!p || !p.local || !p.remote) return "—";
    var fam = p.remote.family || p.local.family;
    return p.local.type + " ↔ " + p.remote.type + " " + (p.local.protocol || "") + "/" + fam;
  }

  /* A rough NAT hint from this browser's own srflx candidates: Chrome keeps
   * one srflx per mapped address, so two ports for one public IP mean the
   * mapping depends on the destination (symmetric-like). */
  function natHint(cands, errors) {
    var srflx = cands.filter(function (c) {
      return c.type === "srflx";
    });
    var v4 = srflx.filter(function (c) {
      return family(c.address) === "IPv4";
    });
    var v6 = srflx.filter(function (c) {
      return family(c.address) === "IPv6";
    });
    var ports = {};
    v4.forEach(function (c) {
      (ports[c.address] = ports[c.address] || {})[c.port] = c.server || "?";
    });
    var failed = {};
    errors.forEach(function (e) {
      STUN.forEach(function (s) {
        if (e.url && e.url.indexOf(s.urls.replace(/^stun:/, "").split(":")[0]) >= 0) failed[s.name] = true;
      });
    });
    var out = {
      srflx_v4: v4.map(function (c) {
        return { port: c.port, server: c.server || null };
      }),
      srflx_v6: v6.length,
      servers_with_errors: Object.keys(failed),
    };
    var tail = v6.length ? "；另有 IPv6 公网地址" : "";
    if (!v4.length) {
      out.code = "none";
      out.text = "IPv4 没拿到 srflx 候选（STUN 不通，或没有 IPv4 出口）" + tail;
      return out;
    }
    var multi = Object.keys(ports).some(function (ip) {
      return Object.keys(ports[ip]).length > 1;
    });
    if (multi) {
      out.code = "symmetric";
      out.text = "IPv4：两台 STUN 看到的端口不同 → 多半是对称型 NAT（打洞难）" + tail;
    } else {
      out.code = "cone";
      out.text =
        "IPv4：只看到一个映射端口 → 多半是锥型 NAT（易打洞" +
        (Object.keys(failed).length ? "；但有 STUN 报错，可能只有一台回应" : "") +
        "）" +
        tail;
    }
    return out;
  }

  function stripCandidates(sdp) {
    return sdp
      .split(/\r\n/)
      .filter(function (l) {
        return l.indexOf("a=candidate:") !== 0 && l !== "a=end-of-candidates";
      })
      .join("\r\n");
  }

  function summaryLine(r) {
    var parts = [r.label_text];
    if (r.direct.connected) {
      parts.push("直连成功 " + (r.direct.connect_ms / 1000).toFixed(2) + " s（" + pairText(r.direct.pair) + "）");
      parts.push("直连 p50 " + fmt(r.dc.p50) + " / p95 " + fmt(r.dc.p95) + " ms");
    } else {
      parts.push("直连失败（" + r.direct.failure + "）");
    }
    parts.push("hub p50 " + fmt(r.hub.p50) + " / p95 " + fmt(r.hub.p95) + " ms");
    parts.push("NAT " + ({ symmetric: "对称型?", cone: "锥型?", none: "无 srflx" }[r.nat.code] || "?"));
    return parts.join(" · ");
  }
  function fmt(x) {
    return x == null ? "—" : x < 10 ? x.toFixed(1) : String(Math.round(x));
  }

  async function run(roomId, label, labelText) {
    var sid = randomHex(8);
    var started = now();
    var since = function () {
      return Math.round(now() - started);
    };
    var result = {
      v: VERSION,
      sid: sid,
      label: label,
      label_text: labelText,
      ua: navigator.userAgent,
      connection: navigator.connection ? { type: navigator.connection.type || null, effective: navigator.connection.effectiveType || null } : null,
      started_at: new Date().toISOString(),
    };

    var room = new Room(roomId, sid);
    var dcPinger = null;
    var hubPinger = new Pinger(function (seq) {
      if (!room.send({ t: "ping", seq: seq })) throw new Error("room closed");
    }, HUB_PING_TIMEOUT_MS);
    room.handlers.pong = function (m, t1) {
      hubPinger.pong(m.seq, t1);
    };
    room.onPeers = function (m) {
      set("v-probe", m.probes ? "在线" : "不在线");
    };

    status("连接 hub 房间…");
    var wsMs = await room.open();
    result.room_open_ms = Math.round(wsMs);
    set("v-room", "已连接（" + Math.round(wsMs) + " ms）");
    await withTimeout(room.peersSeen, 5000, null);
    if (!room.probes) {
      status("电脑上的探针不在线，等 10 秒…");
      for (var i = 0; i < 100 && !room.probes; i++) await sleep(100);
      if (!room.probes) throw new Error("电脑上的探针不在线（rh-p2p-probe 没在跑？）");
    }
    room.send({ t: "hello", label: label });
    var hello = await room.waitFor("hello", 5000);
    if (!hello) throw new Error("探针没有回应");
    set("v-probe", "在线（rh-p2p-probe " + hello.probe + "）");

    /* ICE: trickle both ways. */
    status("建立直连…");
    var cands = [];
    var iceErrors = [];
    var timeline = [];
    var remoteCands = [];
    var remoteSet = false;
    var gatheringDone = null;
    var pc = new RTCPeerConnection({
      iceServers: STUN.map(function (s) {
        return { urls: s.urls };
      }),
      iceTransportPolicy: params.get("nodirect") === "1" ? "relay" : "all",
    });
    if (params.get("nodirect") === "1") result.forced_failure = true;
    var dc = pc.createDataChannel("probe", { ordered: true });
    var dcOpen = new Promise(function (resolve) {
      dc.onopen = function () {
        resolve("open");
      };
    });
    var iceFailed = new Promise(function (resolve) {
      pc.oniceconnectionstatechange = function () {
        timeline.push([pc.iceConnectionState, since()]);
        set("v-direct", "ICE " + pc.iceConnectionState + "…");
        if (pc.iceConnectionState === "failed") resolve("failed");
      };
    });
    pc.onconnectionstatechange = function () {
      timeline.push(["pc:" + pc.connectionState, since()]);
    };
    pc.onicecandidate = function (ev) {
      if (!ev.candidate || !ev.candidate.candidate) {
        if (gatheringDone == null) gatheringDone = since();
        room.send({ t: "cand", c: null });
        return;
      }
      var c = parseCandidate(ev.candidate.candidate);
      var url = ev.url || ev.candidate.url || null;
      if (url) {
        STUN.forEach(function (s) {
          if (url.indexOf(s.urls.replace(/^stun:/, "").split(":")[0]) >= 0) c.server = s.name;
        });
      }
      c.at = since();
      cands.push(c);
      showCands(cands);
      room.send({ t: "cand", c: ev.candidate.candidate });
    };
    pc.onicecandidateerror = function (ev) {
      iceErrors.push({ url: ev.url || null, code: ev.errorCode, text: ev.errorText || null });
    };
    room.handlers.answer = function (m) {
      pc.setRemoteDescription({ type: "answer", sdp: m.sdp })
        .then(function () {
          remoteSet = true;
          timeline.push(["answer", since()]);
          var queued = remoteCands.splice(0);
          queued.forEach(addRemote);
        })
        .catch(function (e) {
          timeline.push(["answer-error:" + e.message, since()]);
        });
    };
    function addRemote(line) {
      pc.addIceCandidate({ candidate: line, sdpMLineIndex: 0 }).catch(function (e) {
        timeline.push(["cand-error:" + e.message, since()]);
      });
    }
    room.handlers.cand = function (m) {
      if (!m.c) return;
      if (remoteSet) addRemote(m.c);
      else remoteCands.push(m.c);
    };
    room.handlers.busy = function () {
      timeline.push(["probe-busy", since()]);
    };

    var t0 = now();
    var offer = await pc.createOffer();
    await pc.setLocalDescription(offer);
    room.send({ t: "offer", label: label, sdp: stripCandidates(pc.localDescription.sdp) });

    var outcome = await withTimeout(Promise.race([dcOpen, iceFailed]), CONNECT_TIMEOUT_MS, "timeout");
    var direct = { connected: outcome === "open", ice_state: pc.iceConnectionState, timeline: timeline };
    if (direct.connected) {
      direct.connect_ms = Math.round(now() - t0);
      direct.pair = await selectedPair(pc);
      set("v-direct", "成功，" + direct.connect_ms + " ms 打通");
      $("v-direct").className = "ok";
      set("v-pair", pairText(direct.pair));
    } else {
      direct.failure = outcome === "failed" ? "ICE failed" : CONNECT_TIMEOUT_MS / 1000 + " s 超时，ICE " + pc.iceConnectionState;
      set("v-direct", "失败：" + direct.failure);
      $("v-direct").className = "bad";
      direct.pair = await selectedPair(pc).catch(function () {
        return null;
      });
    }
    result.direct = direct;

    /* Pings, alternating: direct, then the hub, 200 of each. */
    var dcRtts = [];
    var hubRtts = [];
    var acks = {};
    if (direct.connected) {
      dcPinger = new Pinger(function (seq) {
        dc.send(JSON.stringify({ t: "ping", seq: seq }));
      }, DC_PING_TIMEOUT_MS);
      dc.onmessage = function (ev) {
        var t1 = now();
        var m;
        try {
          m = JSON.parse(ev.data);
        } catch (e) {
          return;
        }
        if (m.t === "pong") dcPinger.pong(m.seq, t1);
        else if (acks[m.t]) acks[m.t](m);
      };
    }
    status("测往返时延…");
    for (var n = 0; n < PINGS; n++) {
      if (dcPinger && dc.readyState === "open") dcRtts.push(await dcPinger.ping());
      else if (dcPinger) dcRtts.push(null);
      hubRtts.push(await hubPinger.ping());
      if (n % 5 === 4 || n === PINGS - 1) {
        if (dcPinger) showPings("dc", dcRtts);
        showPings("hub", hubRtts);
      }
    }
    result.dc = dcPinger ? showPings("dc", dcRtts) : summarize([]);
    result.hub = showPings("hub", hubRtts);
    result.dc_rtts = dcRtts.map(round1);
    result.hub_rtts = hubRtts.map(round1);

    if (gatheringDone == null) await sleep(1000);
    result.gathering_ms = gatheringDone;
    result.candidates = cands;
    result.ice_errors = iceErrors.slice(0, 20);
    result.nat = natHint(cands, iceErrors);
    set("v-nat", result.nat.text);
    if (direct.connected) {
      result.direct.pair_after = await selectedPair(pc);
    }
    result.total_ms = since();
    result.summary = summaryLine(result);

    /* Back to the probe: over the DataChannel, else through the room. */
    status("把结果发回电脑…");
    var saved = null;
    if (direct.connected && dc.readyState === "open") {
      var ack = new Promise(function (resolve) {
        acks.saved = resolve;
      });
      dc.send(JSON.stringify({ t: "result", r: result }));
      saved = (await withTimeout(ack, 3000, null)) ? "直连" : null;
    }
    if (!saved) {
      var small = result;
      if (JSON.stringify(result).length > 7000) {
        small = Object.assign({}, result, { dc_rtts: undefined, hub_rtts: undefined, candidates: cands.slice(0, 12) });
      }
      var hubAck = room.waitFor("saved", 5000);
      room.send({ t: "result", r: small });
      saved = (await hubAck) ? "hub" : null;
    }
    room.send({ t: "bye" });
    try {
      if (direct.connected) dc.send(JSON.stringify({ t: "bye" }));
    } catch (e) {
      /* closed */
    }
    await sleep(100);
    try {
      dc.close();
      pc.close();
    } catch (e) {
      /* closed */
    }
    room.close();
    return { result: result, saved: saved };
  }

  function showCands(cands) {
    var counts = {};
    cands.forEach(function (c) {
      var k = c.type + "/" + family(c.address);
      counts[k] = (counts[k] || 0) + 1;
    });
    set(
      "v-cands",
      Object.keys(counts)
        .map(function (k) {
          return k + "×" + counts[k];
        })
        .join("  ") || "—",
    );
  }

  /* ---------------------------------------------------------------- page */

  var roomId = (location.hash.slice(1).match(/^[0-9a-f]{32}$/) || [])[0];
  var params = new URLSearchParams(location.search);
  var chosen = null;

  var box = $("labels");
  LABELS.forEach(function (l) {
    var lab = document.createElement("label");
    var input = document.createElement("input");
    input.type = "radio";
    input.name = "label";
    input.value = l[0];
    input.onchange = function () {
      chosen = l;
      Array.prototype.forEach.call(box.children, function (el) {
        el.className = "";
      });
      lab.className = "on";
      if (roomId) {
        $("start").disabled = false;
        status("可以开始了");
      }
    };
    lab.appendChild(input);
    lab.appendChild(document.createTextNode(l[1]));
    box.appendChild(lab);
  });

  if (!roomId) {
    status("网址里缺少房间号：请打开 rh-p2p-probe 打印的完整链接（# 后面 32 位）");
  } else {
    set("room-tag", "房间 " + roomId.slice(0, 6) + "…");
  }
  if (typeof RTCPeerConnection === "undefined") {
    status("这个浏览器不支持 WebRTC");
    roomId = null;
  }

  $("again").onclick = function () {
    location.reload();
  };
  $("start").onclick = function () {
    if (!chosen || !roomId) return;
    $("start").disabled = true;
    Array.prototype.forEach.call(box.querySelectorAll("input"), function (el) {
      el.disabled = true;
    });
    run(roomId, chosen[0], chosen[1])
      .then(function (out) {
        var line = out.result.summary;
        $("summary").className = "summary";
        $("summary").textContent = line;
        document.title = "直连探测：完成";
        status(out.saved ? "完成，结果已存到电脑（经" + out.saved + "）" : "完成，但结果没送回电脑");
        window.__probeResult = out;
      })
      .catch(function (e) {
        status("出错：" + e.message);
        $("summary").className = "summary";
        $("summary").textContent = (chosen ? chosen[1] + " · " : "") + "没测成：" + e.message;
        document.title = "直连探测：出错";
        window.__probeResult = { error: e.message };
      })
      .then(function () {
        $("again").hidden = false;
      });
  };

  var auto = params.get("label");
  if (auto) {
    var match = box.querySelector('input[value="' + auto.replace(/[^a-z-]/g, "") + '"]');
    if (match) {
      match.checked = true;
      match.onchange();
      if (params.get("auto") === "1") $("start").onclick();
    }
  }
})();
