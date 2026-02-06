// WHIP/WHEP Demo — publish canvas+oscillator via WHIP, subscribe via WHEP.
//
// No janus.js dependency — uses plain RTCPeerConnection + fetch().
// WHIP (RFC 9725): POST SDP offer to /whip → 201 + SDP answer + Location header
// WHEP: POST SDP offer to /whep/{publisher_id} → 201 + SDP answer + Location header
// Trickle ICE: PATCH /resource/{id} with application/trickle-ice-sdpfrag
// Teardown: DELETE /resource/{id}

"use strict";

var WHIP_URL = "/whip";
var WHEP_URL = "/whep";

// ICE servers for candidate gathering (Google + Cloudflare STUN)
var ICE_SERVERS = [
    { urls: "stun:stun.l.google.com:19302" },
    { urls: "stun:stun.cloudflare.com:3478" }
];

var publisherResourceUrl = null;
var publisherResourceId = null;
var subscriberResourceUrl = null;
var publisherPC = null;
var subscriberPC = null;
var animationId = null;

// ── Logging ──

function log(msg, cls) {
    var el = document.getElementById("status");
    var line = document.createElement("div");
    line.className = cls || "info";
    line.textContent = "[" + new Date().toLocaleTimeString() + "] " + msg;
    el.prepend(line);
}

// ── Canvas bouncing ball (synthetic video source) ──

var ball = { x: 160, y: 120, dx: 2.5, dy: 1.8, r: 18, hue: 0 };

function startCanvas() {
    var canvas = document.getElementById("ball-canvas");
    var ctx = canvas.getContext("2d");

    function draw() {
        ctx.fillStyle = "#111";
        ctx.fillRect(0, 0, canvas.width, canvas.height);

        ball.x += ball.dx;
        ball.y += ball.dy;
        if (ball.x - ball.r < 0 || ball.x + ball.r > canvas.width) ball.dx = -ball.dx;
        if (ball.y - ball.r < 0 || ball.y + ball.r > canvas.height) ball.dy = -ball.dy;
        ball.hue = (ball.hue + 1) % 360;

        ctx.beginPath();
        ctx.arc(ball.x, ball.y, ball.r, 0, Math.PI * 2);
        ctx.fillStyle = "hsl(" + ball.hue + ", 80%, 60%)";
        ctx.fill();

        ctx.fillStyle = "#666";
        ctx.font = "11px monospace";
        ctx.fillText(new Date().toISOString().substr(11, 12), 8, canvas.height - 8);

        animationId = requestAnimationFrame(draw);
    }
    draw();

    // Video from canvas
    var stream = canvas.captureStream(30);

    // Audio: 440 Hz sine
    try {
        var audioCtx = new (window.AudioContext || window.webkitAudioContext)();
        var osc = audioCtx.createOscillator();
        osc.type = "sine";
        osc.frequency.value = 440;
        var gain = audioCtx.createGain();
        gain.gain.value = 0.15;
        var dest = audioCtx.createMediaStreamDestination();
        osc.connect(gain);
        gain.connect(dest);
        osc.start();
        dest.stream.getAudioTracks().forEach(function(t) { stream.addTrack(t); });
    } catch (e) {
        log("Audio not available: " + e.message, "info");
    }

    return stream;
}

// ── WHIP Publish ──

function publish() {
    document.getElementById("btn-publish").disabled = true;
    log("Starting WHIP publish...");

    var stream = startCanvas();
    publisherPC = new RTCPeerConnection({ iceServers: ICE_SERVERS });

    publisherPC.oniceconnectionstatechange = function() {
        log("Publisher ICE: " + publisherPC.iceConnectionState, "info");
    };

    // Add all tracks
    stream.getTracks().forEach(function(track) {
        publisherPC.addTrack(track, stream);
    });

    publisherPC.createOffer().then(function(offer) {
        return publisherPC.setLocalDescription(offer);
    }).then(function() {
        return waitForIceGathering(publisherPC);
    }).then(function() {
        return fetch(WHIP_URL, {
            method: "POST",
            headers: { "Content-Type": "application/sdp" },
            body: publisherPC.localDescription.sdp
        });
    }).then(function(resp) {
        if (resp.status !== 201) {
            return resp.text().then(function(t) { throw new Error("WHIP failed: " + resp.status + " " + t); });
        }
        publisherResourceUrl = resp.headers.get("Location");
        publisherResourceId = publisherResourceUrl
            ? publisherResourceUrl.replace("/resource/", "")
            : null;

        var linkHeader = resp.headers.get("Link");
        if (linkHeader) log("ICE servers: " + linkHeader, "info");

        return resp.text();
    }).then(function(answerSdp) {
        log("WHIP answer received, resource: " + publisherResourceUrl, "ok");
        return publisherPC.setRemoteDescription({ type: "answer", sdp: answerSdp });
    }).then(function() {
        var infoEl = document.getElementById("publisher-info");
        infoEl.innerHTML = 'Publisher ID: <code>' + publisherResourceId + '</code>';
        document.getElementById("btn-subscribe").disabled = false;
        document.getElementById("btn-stop").disabled = false;
        log("WHIP publish active!", "ok");
    }).catch(function(e) {
        log("WHIP error: " + e.message, "err");
        document.getElementById("btn-publish").disabled = false;
    });
}

// ── WHEP Subscribe ──

function subscribe() {
    if (!publisherResourceId) {
        log("No publisher to subscribe to", "err");
        return;
    }
    document.getElementById("btn-subscribe").disabled = true;
    log("Starting WHEP subscribe to " + publisherResourceId + "...");

    subscriberPC = new RTCPeerConnection({ iceServers: ICE_SERVERS });

    subscriberPC.oniceconnectionstatechange = function() {
        log("Subscriber ICE: " + subscriberPC.iceConnectionState, "info");
    };

    subscriberPC.ontrack = function(ev) {
        log("Received remote track: " + ev.track.kind, "ok");
        var video = document.getElementById("remote-video");
        if (!video.srcObject) {
            video.srcObject = new MediaStream();
        }
        video.srcObject.addTrack(ev.track);
        document.getElementById("remote-box").style.display = "";
    };

    // recvonly transceivers
    subscriberPC.addTransceiver("video", { direction: "recvonly" });
    subscriberPC.addTransceiver("audio", { direction: "recvonly" });

    subscriberPC.createOffer().then(function(offer) {
        return subscriberPC.setLocalDescription(offer);
    }).then(function() {
        return waitForIceGathering(subscriberPC);
    }).then(function() {
        return fetch(WHEP_URL + "/" + publisherResourceId, {
            method: "POST",
            headers: { "Content-Type": "application/sdp" },
            body: subscriberPC.localDescription.sdp
        });
    }).then(function(resp) {
        if (resp.status !== 201) {
            return resp.text().then(function(t) { throw new Error("WHEP failed: " + resp.status + " " + t); });
        }
        subscriberResourceUrl = resp.headers.get("Location");
        return resp.text();
    }).then(function(answerSdp) {
        log("WHEP answer received, resource: " + subscriberResourceUrl, "ok");
        return subscriberPC.setRemoteDescription({ type: "answer", sdp: answerSdp });
    }).then(function() {
        document.getElementById("btn-stop").disabled = false;
        log("WHEP subscribe active!", "ok");
    }).catch(function(e) {
        log("WHEP error: " + e.message, "err");
        document.getElementById("btn-subscribe").disabled = false;
    });
}

// ── Stop ──

function stopAll() {
    log("Stopping...");

    var promises = [];

    if (subscriberResourceUrl) {
        promises.push(
            fetch(subscriberResourceUrl, { method: "DELETE" })
                .then(function() { log("Subscriber resource deleted", "ok"); })
                .catch(function(e) { log("Failed to delete subscriber: " + e.message, "err"); })
        );
        subscriberResourceUrl = null;
    }
    if (subscriberPC) {
        subscriberPC.close();
        subscriberPC = null;
    }

    if (publisherResourceUrl) {
        promises.push(
            fetch(publisherResourceUrl, { method: "DELETE" })
                .then(function() { log("Publisher resource deleted", "ok"); })
                .catch(function(e) { log("Failed to delete publisher: " + e.message, "err"); })
        );
        publisherResourceUrl = null;
        publisherResourceId = null;
    }
    if (publisherPC) {
        publisherPC.close();
        publisherPC = null;
    }

    if (animationId) {
        cancelAnimationFrame(animationId);
        animationId = null;
    }

    document.getElementById("remote-video").srcObject = null;
    document.getElementById("remote-box").style.display = "none";
    document.getElementById("publisher-info").innerHTML = "";
    document.getElementById("btn-publish").disabled = false;
    document.getElementById("btn-subscribe").disabled = true;
    document.getElementById("btn-stop").disabled = true;

    Promise.all(promises).then(function() {
        log("Stopped", "ok");
    });
}

// ── Helpers ──

function waitForIceGathering(pc, timeoutMs) {
    timeoutMs = timeoutMs || 2000;
    return new Promise(function(resolve) {
        if (pc.iceGatheringState === "complete") {
            resolve();
            return;
        }
        var timer = setTimeout(resolve, timeoutMs);
        pc.addEventListener("icegatheringstatechange", function() {
            if (pc.iceGatheringState === "complete") {
                clearTimeout(timer);
                resolve();
            }
        });
    });
}
