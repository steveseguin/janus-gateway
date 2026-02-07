// VideoRoom bouncing-ball demo for Janus (Rust) server.
// Publishes a canvas-captured bouncing ball + 440 Hz sinewave to room 1234,
// and subscribes to other publishers in the same room.

/* global Janus:readonly, server:readonly */

"use strict";

var janus = null;
var publisherHandle = null;
var myUserId = null;
var remoteHandles = {};
var roomId = 1234;

// ── Bouncing ball animation ──

var canvas = document.getElementById("ball-canvas");
var ctx = canvas.getContext("2d");
var ball = { x: 160, y: 120, dx: 2.5, dy: 1.8, r: 18, hue: 0 };

function drawBall() {
	// Clear
	ctx.fillStyle = "#111";
	ctx.fillRect(0, 0, canvas.width, canvas.height);

	// Update position
	ball.x += ball.dx;
	ball.y += ball.dy;
	if (ball.x - ball.r < 0 || ball.x + ball.r > canvas.width) ball.dx = -ball.dx;
	if (ball.y - ball.r < 0 || ball.y + ball.r > canvas.height) ball.dy = -ball.dy;
	ball.hue = (ball.hue + 1) % 360;

	// Draw
	ctx.beginPath();
	ctx.arc(ball.x, ball.y, ball.r, 0, Math.PI * 2);
	ctx.fillStyle = "hsl(" + ball.hue + ", 80%, 60%)";
	ctx.fill();

	// Timestamp watermark
	ctx.fillStyle = "#666";
	ctx.font = "11px monospace";
	ctx.fillText(new Date().toISOString().substr(11, 12), 8, canvas.height - 8);

	requestAnimationFrame(drawBall);
}
drawBall();

// ── Synthetic media stream ──

function createSyntheticStream() {
	// Video from canvas
	var videoStream = canvas.captureStream(30);

	// Audio: 440 Hz sine wave
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

	// Combine video + audio
	var combined = new MediaStream();
	videoStream.getVideoTracks().forEach(function(t) { combined.addTrack(t); });
	dest.stream.getAudioTracks().forEach(function(t) { combined.addTrack(t); });
	return combined;
}

// ── Status helper ──

function setStatus(msg, isError) {
	var el = document.getElementById("status");
	el.textContent = msg;
	el.className = isError ? "err" : "ok";
}

// ── Janus connection ──

Janus.init({
	debug: "all",
	callback: function() {
		janus = new Janus({
			server: server,
			success: function() {
				setStatus("Connected to Janus, joining room...");
				attachPublisher();
			},
			error: function(err) {
				setStatus("Janus error: " + err, true);
			},
			destroyed: function() {
				setStatus("Janus session destroyed", true);
			}
		});
	}
});

function attachPublisher() {
	janus.attach({
		plugin: "janus.plugin.videoroom",
		success: function(handle) {
			publisherHandle = handle;
			// Join room as publisher
			publisherHandle.send({
				message: {
					request: "join",
					room: roomId,
					ptype: "publisher",
					display: "BouncingBall-" + Janus.randomString(4)
				}
			});
		},
		error: function(err) {
			setStatus("Attach error: " + err, true);
		},
		onmessage: function(msg, jsep) {
			var event = msg.videoroom;
			if (event === "joined") {
				myUserId = msg.id;
				setStatus("Joined room " + roomId + " as user " + myUserId);
				publishStream();

				// Subscribe to existing publishers
				if (msg.publishers && msg.publishers.length > 0) {
					msg.publishers.forEach(function(pub) {
						subscribeToFeed(pub.id, pub.display);
					});
				}
			} else if (event === "event") {
				// New publisher joined
				if (msg.publishers && msg.publishers.length > 0) {
					msg.publishers.forEach(function(pub) {
						subscribeToFeed(pub.id, pub.display);
					});
				}
				// Publisher left
				if (msg.unpublished) {
					removeRemoteFeed(msg.unpublished);
				}
				if (msg.leaving) {
					removeRemoteFeed(msg.leaving);
				}
			}

			if (jsep) {
				publisherHandle.handleRemoteJsep({ jsep: jsep });
			}
		},
		onlocalstream: function(stream) {
			// We don't display the local stream separately — canvas is our preview
		},
		onremotestream: function(stream) {
			// Publisher handle shouldn't receive remote streams
		}
	});
}

function publishStream() {
	var stream = createSyntheticStream();
	publisherHandle.createOffer({
		stream: stream,
		success: function(jsep) {
			publisherHandle.send({
				message: { request: "publish", audio: true, video: true },
				jsep: jsep
			});
			setStatus("Publishing bouncing ball + audio to room " + roomId);
		},
		error: function(err) {
			setStatus("Publish error: " + err, true);
		}
	});
}

// ── Subscribe to remote feeds ──

function subscribeToFeed(feedId, display) {
	if (remoteHandles[feedId]) return; // already subscribed

	janus.attach({
		plugin: "janus.plugin.videoroom",
		success: function(handle) {
			remoteHandles[feedId] = handle;
			handle.send({
				message: {
					request: "join",
					room: roomId,
					ptype: "subscriber",
					feed: feedId
				}
			});
		},
		error: function(err) {
			console.error("Subscribe error:", err);
		},
		onmessage: function(msg, jsep) {
			if (jsep) {
				remoteHandles[feedId].createAnswer({
					jsep: jsep,
					media: { audioSend: false, videoSend: false },
					success: function(ourJsep) {
						remoteHandles[feedId].send({
							message: { request: "start" },
							jsep: ourJsep
						});
					},
					error: function(err) {
						console.error("createAnswer error:", err);
					}
				});
			}
		},
		onremotestream: function(stream) {
			addRemoteVideo(feedId, display || "User " + feedId, stream);
		},
		oncleanup: function() {
			removeRemoteFeed(feedId);
		}
	});
}

function addRemoteVideo(feedId, display, stream) {
	var container = document.getElementById("remote-feeds");
	var existing = document.getElementById("remote-" + feedId);
	if (existing) return;

	var box = document.createElement("div");
	box.className = "video-box";
	box.id = "remote-" + feedId;

	var title = document.createElement("h3");
	title.textContent = "Remote: " + display;
	box.appendChild(title);

	var video = document.createElement("video");
	video.autoplay = true;
	video.playsInline = true;
	video.srcObject = stream;
	box.appendChild(video);

	container.appendChild(box);
}

function removeRemoteFeed(feedId) {
	var el = document.getElementById("remote-" + feedId);
	if (el) el.remove();
	if (remoteHandles[feedId]) {
		remoteHandles[feedId].detach();
		delete remoteHandles[feedId];
	}
}
