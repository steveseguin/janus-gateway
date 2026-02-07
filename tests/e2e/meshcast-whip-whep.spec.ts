import { test, expect } from "@playwright/test";

// Meshcast WHIP/WHEP endpoints
const MESHCAST_WHIP_URL =
  "https://app.meshcast.io/api/gateway/whip/pk_UVqz7jBRZG1xQjql6ZIdoA";
const MESHCAST_WHEP_URL =
  "https://app.meshcast.io/api/gateway/whep/st__gq7JbZUZ_pQ";

// ICE servers for candidate gathering
const ICE_SERVERS = [
  { urls: "stun:stun.l.google.com:19302" },
  { urls: "stun:stun.cloudflare.com:3478" },
];

// Track resource URLs for cleanup
let resourceUrls: string[] = [];

// Skip all tests if Meshcast is unreachable
let meshcastAvailable = false;

test.beforeAll(async ({ browser }) => {
  const page = await browser.newPage();
  try {
    const reachable = await page.evaluate(async (url: string) => {
      try {
        const ctrl = new AbortController();
        const timer = setTimeout(() => ctrl.abort(), 10000);
        const resp = await fetch(url, {
          method: "OPTIONS",
          signal: ctrl.signal,
        });
        clearTimeout(timer);
        return resp.status < 500;
      } catch {
        return false;
      }
    }, MESHCAST_WHIP_URL);
    meshcastAvailable = reachable;
  } catch {
    meshcastAvailable = false;
  } finally {
    await page.close();
  }
});

test.beforeEach(async () => {
  resourceUrls = [];
});

test.afterEach(async ({ page }) => {
  // Clean up any Meshcast resources created during the test
  for (const url of resourceUrls) {
    try {
      await page.evaluate(async (resourceUrl: string) => {
        try {
          await fetch(resourceUrl, { method: "DELETE" });
        } catch {
          // ignore cleanup errors
        }
      }, url);
    } catch {
      // ignore
    }
  }
});

/**
 * Helper: create a canvas bouncing-ball + sine-tone MediaStream inside the browser.
 * Returns the stream and an animation-stop callback.
 */
function createMediaStreamScript() {
  return `
    (() => {
      const canvas = document.createElement('canvas');
      canvas.width = 320;
      canvas.height = 240;
      document.body.appendChild(canvas);
      const ctx = canvas.getContext('2d');

      let ball = { x: 160, y: 120, dx: 2.5, dy: 1.8, r: 18, hue: 0 };
      let animId = null;

      function draw() {
        ctx.fillStyle = '#111';
        ctx.fillRect(0, 0, 320, 240);
        ball.x += ball.dx;
        ball.y += ball.dy;
        if (ball.x - ball.r < 0 || ball.x + ball.r > 320) ball.dx = -ball.dx;
        if (ball.y - ball.r < 0 || ball.y + ball.r > 240) ball.dy = -ball.dy;
        ball.hue = (ball.hue + 1) % 360;
        ctx.beginPath();
        ctx.arc(ball.x, ball.y, ball.r, 0, Math.PI * 2);
        ctx.fillStyle = 'hsl(' + ball.hue + ', 80%, 60%)';
        ctx.fill();
        animId = requestAnimationFrame(draw);
      }
      draw();

      const stream = canvas.captureStream(30);

      try {
        const audioCtx = new AudioContext();
        const osc = audioCtx.createOscillator();
        osc.type = 'sine';
        osc.frequency.value = 440;
        const gain = audioCtx.createGain();
        gain.gain.value = 0.15;
        const dest = audioCtx.createMediaStreamDestination();
        osc.connect(gain);
        gain.connect(dest);
        osc.start();
        dest.stream.getAudioTracks().forEach(t => stream.addTrack(t));
      } catch (e) {
        // audio not available — ok
      }

      window.__testStream = stream;
      window.__stopAnimation = () => {
        if (animId) cancelAnimationFrame(animId);
      };
    })();
  `;
}

test.describe("Meshcast WHIP/WHEP E2E", () => {
  test.beforeEach(async () => {
    test.skip(!meshcastAvailable, "Meshcast unreachable — skipping");
  });

  test("WHIP publish reaches ICE connected", async ({ page }) => {
    await page.goto("about:blank");

    const result = await page.evaluate(
      async ({
        whipUrl,
        iceServers,
        createStreamScript,
      }: {
        whipUrl: string;
        iceServers: { urls: string }[];
        createStreamScript: string;
      }) => {
        // Create media stream
        eval(createStreamScript);
        const stream = (window as any).__testStream as MediaStream;

        const pc = new RTCPeerConnection({ iceServers });

        // Add tracks
        stream.getTracks().forEach((track) => pc.addTrack(track, stream));

        // Create offer
        const offer = await pc.createOffer();
        await pc.setLocalDescription(offer);

        // Wait for ICE gathering to complete (5s timeout)
        await new Promise<void>((resolve) => {
          if (pc.iceGatheringState === "complete") {
            resolve();
            return;
          }
          const timer = setTimeout(resolve, 5000);
          pc.addEventListener("icegatheringstatechange", () => {
            if (pc.iceGatheringState === "complete") {
              clearTimeout(timer);
              resolve();
            }
          });
        });

        // POST to Meshcast WHIP
        const resp = await fetch(whipUrl, {
          method: "POST",
          headers: { "Content-Type": "application/sdp" },
          body: pc.localDescription!.sdp,
        });

        const status = resp.status;
        const location = resp.headers.get("Location");
        const etag = resp.headers.get("ETag");
        const answerSdp = await resp.text();

        if (status !== 201) {
          pc.close();
          return { status, location, etag, answerSdp, iceState: "failed" };
        }

        // Apply answer
        await pc.setRemoteDescription({ type: "answer", sdp: answerSdp });

        // Wait for ICE connected/completed (15s timeout)
        const iceState = await new Promise<string>((resolve) => {
          if (
            pc.iceConnectionState === "connected" ||
            pc.iceConnectionState === "completed"
          ) {
            resolve(pc.iceConnectionState);
            return;
          }
          const timer = setTimeout(
            () => resolve(pc.iceConnectionState),
            15000
          );
          pc.addEventListener("iceconnectionstatechange", () => {
            if (
              pc.iceConnectionState === "connected" ||
              pc.iceConnectionState === "completed"
            ) {
              clearTimeout(timer);
              resolve(pc.iceConnectionState);
            } else if (pc.iceConnectionState === "failed") {
              clearTimeout(timer);
              resolve("failed");
            }
          });
        });

        pc.close();
        (window as any).__stopAnimation?.();

        return { status, location, etag, answerSdp, iceState };
      },
      {
        whipUrl: MESHCAST_WHIP_URL,
        iceServers: ICE_SERVERS,
        createStreamScript: createMediaStreamScript(),
      }
    );

    // Track for cleanup
    if (result.location) {
      // Meshcast location may be relative or absolute
      const fullUrl = result.location.startsWith("http")
        ? result.location
        : new URL(result.location, MESHCAST_WHIP_URL).href;
      resourceUrls.push(fullUrl);
    }

    expect(result.status).toBe(201);
    expect(result.location).toBeTruthy();
    expect(result.etag).toBeTruthy();
    expect(result.answerSdp).toContain("a=setup:");
    expect(result.answerSdp).toContain("a=ice-ufrag:");
    expect(["connected", "completed"]).toContain(result.iceState);
  });

  test("WHEP subscribe receives video frames", async ({ page }) => {
    // This test depends on an active publisher on the Meshcast stream.
    // If nobody is publishing, WHEP may fail or yield no frames — skip gracefully.
    await page.goto("about:blank");

    const result = await page.evaluate(
      async ({
        whepUrl,
        iceServers,
      }: {
        whepUrl: string;
        iceServers: { urls: string }[];
      }) => {
        const pc = new RTCPeerConnection({ iceServers });

        let gotVideoTrack = false;
        pc.ontrack = (ev) => {
          if (ev.track.kind === "video") gotVideoTrack = true;
        };

        // recvonly transceivers
        pc.addTransceiver("video", { direction: "recvonly" });
        pc.addTransceiver("audio", { direction: "recvonly" });

        const offer = await pc.createOffer();
        await pc.setLocalDescription(offer);

        // Wait for ICE gathering
        await new Promise<void>((resolve) => {
          if (pc.iceGatheringState === "complete") {
            resolve();
            return;
          }
          const timer = setTimeout(resolve, 5000);
          pc.addEventListener("icegatheringstatechange", () => {
            if (pc.iceGatheringState === "complete") {
              clearTimeout(timer);
              resolve();
            }
          });
        });

        // POST to Meshcast WHEP
        const resp = await fetch(whepUrl, {
          method: "POST",
          headers: { "Content-Type": "application/sdp" },
          body: pc.localDescription!.sdp,
        });

        const status = resp.status;
        const location = resp.headers.get("Location");

        if (status !== 201) {
          pc.close();
          return {
            status,
            location,
            iceState: "skipped",
            gotVideoTrack: false,
            decodedFrames: 0,
            skipped: true,
            skipReason: "WHEP returned " + status + " (no active publisher?)",
          };
        }

        const answerSdp = await resp.text();
        await pc.setRemoteDescription({ type: "answer", sdp: answerSdp });

        // Wait for ICE connected (15s)
        const iceState = await new Promise<string>((resolve) => {
          if (
            pc.iceConnectionState === "connected" ||
            pc.iceConnectionState === "completed"
          ) {
            resolve(pc.iceConnectionState);
            return;
          }
          const timer = setTimeout(
            () => resolve(pc.iceConnectionState),
            15000
          );
          pc.addEventListener("iceconnectionstatechange", () => {
            if (
              pc.iceConnectionState === "connected" ||
              pc.iceConnectionState === "completed"
            ) {
              clearTimeout(timer);
              resolve(pc.iceConnectionState);
            } else if (pc.iceConnectionState === "failed") {
              clearTimeout(timer);
              resolve("failed");
            }
          });
        });

        // Wait for video frames (8s)
        let decodedFrames = 0;
        if (
          (iceState === "connected" || iceState === "completed") &&
          gotVideoTrack
        ) {
          const receivers = pc.getReceivers();
          const videoReceiver = receivers.find(
            (r) => r.track && r.track.kind === "video"
          );
          if (videoReceiver) {
            // Use getStats to count decoded frames
            await new Promise<void>((resolve) => {
              const start = Date.now();
              const check = async () => {
                const stats = await pc.getStats(videoReceiver.track);
                stats.forEach((stat: any) => {
                  if (
                    stat.type === "inbound-rtp" &&
                    stat.kind === "video" &&
                    stat.framesDecoded
                  ) {
                    decodedFrames = stat.framesDecoded;
                  }
                });
                if (decodedFrames > 0 || Date.now() - start > 8000) {
                  resolve();
                } else {
                  setTimeout(check, 500);
                }
              };
              check();
            });
          }
        }

        pc.close();

        return {
          status,
          location,
          iceState,
          gotVideoTrack,
          decodedFrames,
          skipped: false,
          skipReason: "",
        };
      },
      { whepUrl: MESHCAST_WHEP_URL, iceServers: ICE_SERVERS }
    );

    if (result.location) {
      const fullUrl = result.location.startsWith("http")
        ? result.location
        : new URL(result.location, MESHCAST_WHEP_URL).href;
      resourceUrls.push(fullUrl);
    }

    if (result.skipped) {
      test.skip(true, result.skipReason);
      return;
    }

    expect(result.status).toBe(201);
    expect(["connected", "completed"]).toContain(result.iceState);
    expect(result.gotVideoTrack).toBe(true);
    expect(result.decodedFrames).toBeGreaterThanOrEqual(1);
  });

  test("Round-trip — WHIP publish then WHEP subscribe", async ({ page }) => {
    test.setTimeout(120_000);
    await page.goto("about:blank");

    const result = await page.evaluate(
      async ({
        whipUrl,
        whepUrl,
        iceServers,
        createStreamScript,
      }: {
        whipUrl: string;
        whepUrl: string;
        iceServers: { urls: string }[];
        createStreamScript: string;
      }) => {
        // ── Phase 1: WHIP publish ──
        eval(createStreamScript);
        const stream = (window as any).__testStream as MediaStream;

        const pubPC = new RTCPeerConnection({ iceServers });
        stream.getTracks().forEach((track) => pubPC.addTrack(track, stream));

        const pubOffer = await pubPC.createOffer();
        await pubPC.setLocalDescription(pubOffer);

        // Wait for ICE gathering (5s)
        await new Promise<void>((resolve) => {
          if (pubPC.iceGatheringState === "complete") {
            resolve();
            return;
          }
          const timer = setTimeout(resolve, 5000);
          pubPC.addEventListener("icegatheringstatechange", () => {
            if (pubPC.iceGatheringState === "complete") {
              clearTimeout(timer);
              resolve();
            }
          });
        });

        const whipResp = await fetch(whipUrl, {
          method: "POST",
          headers: { "Content-Type": "application/sdp" },
          body: pubPC.localDescription!.sdp,
        });

        const whipStatus = whipResp.status;
        const whipLocation = whipResp.headers.get("Location");

        if (whipStatus !== 201) {
          pubPC.close();
          (window as any).__stopAnimation?.();
          return {
            whipStatus,
            whipLocation,
            pubIceState: "failed",
            whepStatus: 0,
            whepLocation: null as string | null,
            subIceState: "skipped",
            gotVideoTrack: false,
            decodedFrames: 0,
          };
        }

        const whipAnswer = await whipResp.text();
        await pubPC.setRemoteDescription({ type: "answer", sdp: whipAnswer });

        // Wait for publisher ICE connected (15s)
        const pubIceState = await new Promise<string>((resolve) => {
          if (
            pubPC.iceConnectionState === "connected" ||
            pubPC.iceConnectionState === "completed"
          ) {
            resolve(pubPC.iceConnectionState);
            return;
          }
          const timer = setTimeout(
            () => resolve(pubPC.iceConnectionState),
            15000
          );
          pubPC.addEventListener("iceconnectionstatechange", () => {
            if (
              pubPC.iceConnectionState === "connected" ||
              pubPC.iceConnectionState === "completed"
            ) {
              clearTimeout(timer);
              resolve(pubPC.iceConnectionState);
            } else if (pubPC.iceConnectionState === "failed") {
              clearTimeout(timer);
              resolve("failed");
            }
          });
        });

        if (pubIceState !== "connected" && pubIceState !== "completed") {
          pubPC.close();
          (window as any).__stopAnimation?.();
          return {
            whipStatus,
            whipLocation,
            pubIceState,
            whepStatus: 0,
            whepLocation: null as string | null,
            subIceState: "skipped",
            gotVideoTrack: false,
            decodedFrames: 0,
          };
        }

        // Let the publisher settle for 2s so Meshcast has media flowing
        await new Promise((r) => setTimeout(r, 2000));

        // ── Phase 2: WHEP subscribe ──
        const subPC = new RTCPeerConnection({ iceServers });

        let gotVideoTrack = false;
        subPC.ontrack = (ev) => {
          if (ev.track.kind === "video") gotVideoTrack = true;
        };

        subPC.addTransceiver("video", { direction: "recvonly" });
        subPC.addTransceiver("audio", { direction: "recvonly" });

        const subOffer = await subPC.createOffer();
        await subPC.setLocalDescription(subOffer);

        // Wait for ICE gathering (5s)
        await new Promise<void>((resolve) => {
          if (subPC.iceGatheringState === "complete") {
            resolve();
            return;
          }
          const timer = setTimeout(resolve, 5000);
          subPC.addEventListener("icegatheringstatechange", () => {
            if (subPC.iceGatheringState === "complete") {
              clearTimeout(timer);
              resolve();
            }
          });
        });

        const whepResp = await fetch(whepUrl, {
          method: "POST",
          headers: { "Content-Type": "application/sdp" },
          body: subPC.localDescription!.sdp,
        });

        const whepStatus = whepResp.status;
        const whepLocation = whepResp.headers.get("Location");

        if (whepStatus !== 201) {
          subPC.close();
          pubPC.close();
          (window as any).__stopAnimation?.();
          return {
            whipStatus,
            whipLocation,
            pubIceState,
            whepStatus,
            whepLocation,
            subIceState: "failed",
            gotVideoTrack: false,
            decodedFrames: 0,
          };
        }

        const whepAnswer = await whepResp.text();
        await subPC.setRemoteDescription({ type: "answer", sdp: whepAnswer });

        // Wait for subscriber ICE connected (15s)
        const subIceState = await new Promise<string>((resolve) => {
          if (
            subPC.iceConnectionState === "connected" ||
            subPC.iceConnectionState === "completed"
          ) {
            resolve(subPC.iceConnectionState);
            return;
          }
          const timer = setTimeout(
            () => resolve(subPC.iceConnectionState),
            15000
          );
          subPC.addEventListener("iceconnectionstatechange", () => {
            if (
              subPC.iceConnectionState === "connected" ||
              subPC.iceConnectionState === "completed"
            ) {
              clearTimeout(timer);
              resolve(subPC.iceConnectionState);
            } else if (subPC.iceConnectionState === "failed") {
              clearTimeout(timer);
              resolve("failed");
            }
          });
        });

        // Wait for video frames (8s)
        let decodedFrames = 0;
        if (
          (subIceState === "connected" || subIceState === "completed") &&
          gotVideoTrack
        ) {
          const receivers = subPC.getReceivers();
          const videoReceiver = receivers.find(
            (r) => r.track && r.track.kind === "video"
          );
          if (videoReceiver) {
            await new Promise<void>((resolve) => {
              const start = Date.now();
              const check = async () => {
                const stats = await subPC.getStats(videoReceiver.track);
                stats.forEach((stat: any) => {
                  if (
                    stat.type === "inbound-rtp" &&
                    stat.kind === "video" &&
                    stat.framesDecoded
                  ) {
                    decodedFrames = stat.framesDecoded;
                  }
                });
                if (decodedFrames > 0 || Date.now() - start > 8000) {
                  resolve();
                } else {
                  setTimeout(check, 500);
                }
              };
              check();
            });
          }
        }

        // Cleanup PeerConnections
        subPC.close();
        pubPC.close();
        (window as any).__stopAnimation?.();

        return {
          whipStatus,
          whipLocation,
          pubIceState,
          whepStatus,
          whepLocation,
          subIceState,
          gotVideoTrack,
          decodedFrames,
        };
      },
      {
        whipUrl: MESHCAST_WHIP_URL,
        whepUrl: MESHCAST_WHEP_URL,
        iceServers: ICE_SERVERS,
        createStreamScript: createMediaStreamScript(),
      }
    );

    // Track for cleanup
    if (result.whipLocation) {
      const fullUrl = result.whipLocation.startsWith("http")
        ? result.whipLocation
        : new URL(result.whipLocation, MESHCAST_WHIP_URL).href;
      resourceUrls.push(fullUrl);
    }
    if (result.whepLocation) {
      const fullUrl = result.whepLocation.startsWith("http")
        ? result.whepLocation
        : new URL(result.whepLocation, MESHCAST_WHEP_URL).href;
      resourceUrls.push(fullUrl);
    }

    expect(result.whipStatus).toBe(201);
    expect(["connected", "completed"]).toContain(result.pubIceState);
    expect(result.whepStatus).toBe(201);
    expect(["connected", "completed"]).toContain(result.subIceState);
    expect(result.gotVideoTrack).toBe(true);
    expect(result.decodedFrames).toBeGreaterThanOrEqual(1);
  });

  test("DELETE cleanup works", async ({ page }) => {
    await page.goto("about:blank");

    const result = await page.evaluate(
      async ({
        whipUrl,
        iceServers,
        createStreamScript,
      }: {
        whipUrl: string;
        iceServers: { urls: string }[];
        createStreamScript: string;
      }) => {
        // Create a minimal WHIP resource
        eval(createStreamScript);
        const stream = (window as any).__testStream as MediaStream;

        const pc = new RTCPeerConnection({ iceServers });
        stream.getTracks().forEach((track) => pc.addTrack(track, stream));

        const offer = await pc.createOffer();
        await pc.setLocalDescription(offer);

        // Wait for ICE gathering (5s)
        await new Promise<void>((resolve) => {
          if (pc.iceGatheringState === "complete") {
            resolve();
            return;
          }
          const timer = setTimeout(resolve, 5000);
          pc.addEventListener("icegatheringstatechange", () => {
            if (pc.iceGatheringState === "complete") {
              clearTimeout(timer);
              resolve();
            }
          });
        });

        const resp = await fetch(whipUrl, {
          method: "POST",
          headers: { "Content-Type": "application/sdp" },
          body: pc.localDescription!.sdp,
        });

        const createStatus = resp.status;
        const location = resp.headers.get("Location");

        if (createStatus !== 201 || !location) {
          pc.close();
          (window as any).__stopAnimation?.();
          return { createStatus, location, deleteStatus: 0 };
        }

        // Apply answer so the resource is fully established
        const answerSdp = await resp.text();
        await pc.setRemoteDescription({ type: "answer", sdp: answerSdp });

        // Build absolute DELETE URL
        const deleteUrl = location.startsWith("http")
          ? location
          : new URL(location, whipUrl).href;

        // DELETE the resource
        const delResp = await fetch(deleteUrl, { method: "DELETE" });
        const deleteStatus = delResp.status;

        pc.close();
        (window as any).__stopAnimation?.();

        return { createStatus, location, deleteStatus };
      },
      {
        whipUrl: MESHCAST_WHIP_URL,
        iceServers: ICE_SERVERS,
        createStreamScript: createMediaStreamScript(),
      }
    );

    // No need to add to resourceUrls — we already deleted it in the test

    expect(result.createStatus).toBe(201);
    expect(result.location).toBeTruthy();
    expect([200, 204]).toContain(result.deleteStatus);
  });
});
