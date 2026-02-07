// Streaming viewer for Janus (Rust) server.
// Connects to mountpoint 1 and displays the stream.

/* global Janus:readonly, server:readonly */

"use strict";

var janus = null;
var streamingHandle = null;
var mountpointId = 1;

function setStatus(msg, isError) {
	var el = document.getElementById("status");
	el.textContent = msg;
	el.className = isError ? "err" : "ok";
}

Janus.init({
	debug: "all",
	callback: function() {
		janus = new Janus({
			server: server,
			success: function() {
				setStatus("Connected to Janus, attaching to streaming plugin...");
				attachStreaming();
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

function attachStreaming() {
	janus.attach({
		plugin: "janus.plugin.streaming",
		success: function(handle) {
			streamingHandle = handle;
			// Request to watch mountpoint
			streamingHandle.send({
				message: {
					request: "watch",
					id: mountpointId
				}
			});
			setStatus("Requesting stream from mountpoint " + mountpointId + "...");
		},
		error: function(err) {
			setStatus("Attach error: " + err, true);
		},
		onmessage: function(msg, jsep) {
			if (jsep) {
				// Server sent an offer, create answer
				streamingHandle.createAnswer({
					jsep: jsep,
					media: { audioSend: false, videoSend: false },
					success: function(ourJsep) {
						streamingHandle.send({
							message: { request: "start" },
							jsep: ourJsep
						});
						setStatus("Stream started from mountpoint " + mountpointId);
					},
					error: function(err) {
						setStatus("createAnswer error: " + err, true);
					}
				});
			}

			var result = msg.result;
			if (result) {
				if (result.status === "stopped") {
					setStatus("Stream stopped");
				}
			}
		},
		onremotestream: function(stream) {
			var video = document.getElementById("stream-video");
			video.srcObject = stream;
			setStatus("Receiving stream from mountpoint " + mountpointId);
		},
		oncleanup: function() {
			setStatus("Stream cleaned up");
		}
	});
}
