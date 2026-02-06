// Standalone WHEP Player — subscribe to any WHIP publisher by ID.
//
// No janus.js dependency — uses plain RTCPeerConnection + fetch().
// WHEP: POST SDP offer to /whep/{publisher_id} → 201 + SDP answer + Location header
// Teardown: DELETE /resource/{id}

"use strict";

var WHEP_URL = "/whep";

// ICE servers for candidate gathering (Google + Cloudflare STUN)
var ICE_SERVERS = [
    { urls: "stun:stun.l.google.com:19302" },
    { urls: "stun:stun.cloudflare.com:3478" }
];

var subscriberPC = null;
var subscriberResourceUrl = null;

// ── Logging ──

function log(msg, cls) {
    var el = document.getElementById("status");
    var line = document.createElement("div");
    line.className = cls || "info";
    line.textContent = "[" + new Date().toLocaleTimeString() + "] " + msg;
    el.prepend(line);
}

// ── Watch (WHEP Subscribe) ──

function watch() {
    var publisherId = document.getElementById("publisher-id").value.trim();
    if (!publisherId) {
        log("Please enter a Publisher ID", "err");
        return;
    }

    document.getElementById("btn-watch").disabled = true;
    log("Starting WHEP subscribe to " + publisherId + "...");

    subscriberPC = new RTCPeerConnection({ iceServers: ICE_SERVERS });

    subscriberPC.oniceconnectionstatechange = function() {
        log("ICE: " + subscriberPC.iceConnectionState, "info");
        if (subscriberPC.iceConnectionState === "failed" ||
            subscriberPC.iceConnectionState === "disconnected") {
            log("Connection lost", "err");
        }
    };

    subscriberPC.ontrack = function(ev) {
        log("Received remote track: " + ev.track.kind, "ok");
        var video = document.getElementById("remote-video");
        if (!video.srcObject) {
            video.srcObject = new MediaStream();
        }
        video.srcObject.addTrack(ev.track);
    };

    // recvonly transceivers
    subscriberPC.addTransceiver("video", { direction: "recvonly" });
    subscriberPC.addTransceiver("audio", { direction: "recvonly" });

    subscriberPC.createOffer().then(function(offer) {
        return subscriberPC.setLocalDescription(offer);
    }).then(function() {
        return waitForIceGathering(subscriberPC);
    }).then(function() {
        return fetch(WHEP_URL + "/" + publisherId, {
            method: "POST",
            headers: { "Content-Type": "application/sdp" },
            body: subscriberPC.localDescription.sdp
        });
    }).then(function(resp) {
        if (resp.status !== 201) {
            return resp.text().then(function(t) { throw new Error("WHEP failed: " + resp.status + " " + t); });
        }
        subscriberResourceUrl = resp.headers.get("Location");

        var linkHeader = resp.headers.get("Link");
        if (linkHeader) log("ICE servers: " + linkHeader, "info");

        return resp.text();
    }).then(function(answerSdp) {
        log("WHEP answer received, resource: " + subscriberResourceUrl, "ok");
        return subscriberPC.setRemoteDescription({ type: "answer", sdp: answerSdp });
    }).then(function() {
        document.getElementById("btn-stop").disabled = false;
        log("WHEP subscribe active!", "ok");
    }).catch(function(e) {
        log("WHEP error: " + e.message, "err");
        document.getElementById("btn-watch").disabled = false;
        if (subscriberPC) {
            subscriberPC.close();
            subscriberPC = null;
        }
    });
}

// ── Stop ──

function stop() {
    log("Stopping...");

    if (subscriberResourceUrl) {
        fetch(subscriberResourceUrl, { method: "DELETE" })
            .then(function() { log("Subscriber resource deleted", "ok"); })
            .catch(function(e) { log("Failed to delete: " + e.message, "err"); });
        subscriberResourceUrl = null;
    }

    if (subscriberPC) {
        subscriberPC.close();
        subscriberPC = null;
    }

    document.getElementById("remote-video").srcObject = null;
    document.getElementById("btn-watch").disabled = false;
    document.getElementById("btn-stop").disabled = true;

    log("Stopped", "ok");
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

// ── Auto-fill from URL hash ──

(function() {
    var hash = window.location.hash.replace("#", "");
    if (hash) {
        document.getElementById("publisher-id").value = hash;
        log("Publisher ID loaded from URL: " + hash, "info");
    }
})();
