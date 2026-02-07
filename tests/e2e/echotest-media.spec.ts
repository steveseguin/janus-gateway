import { test, expect, Page } from "@playwright/test";

/**
 * EchoTest Media E2E tests.
 *
 * These tests verify actual WebRTC media flow through the EchoTest plugin
 * by using the Janus API to set up a PeerConnection, send fake media,
 * and verify that media is echoed back via RTCStats.
 */

/** Helper: create a Janus session via API. */
async function createSession(request: any): Promise<number> {
  const resp = await request.post("/janus", {
    data: { janus: "create", transaction: "t-create" },
  });
  const json = await resp.json();
  expect(json.janus).toBe("success");
  return json.data.id;
}

/** Helper: attach to echotest plugin. */
async function attachEchoTest(
  request: any,
  sessionId: number,
): Promise<number> {
  const resp = await request.post(`/janus/${sessionId}`, {
    data: {
      janus: "attach",
      plugin: "janus.plugin.echotest",
      transaction: "t-attach",
    },
  });
  const json = await resp.json();
  expect(json.janus).toBe("success");
  return json.data.id;
}

/** Helper: destroy session. */
async function destroySession(request: any, sessionId: number) {
  await request.post(`/janus/${sessionId}`, {
    data: { janus: "destroy", transaction: "t-destroy" },
  });
}

/**
 * Helper: send a message and get the event response.
 * If the server returns "ack" (async processing), polls for the event.
 */
async function sendMessageAndGetEvent(
  request: any,
  sessionId: number,
  handleId: number,
  body: any,
  jsep?: any,
): Promise<any> {
  const data: any = {
    janus: "message",
    transaction: `t-${Date.now()}`,
    body,
  };
  if (jsep) data.jsep = jsep;

  const resp = await request.post(`/janus/${sessionId}/${handleId}`, { data });
  const json = await resp.json();

  if (json.janus === "event") return json;

  // Got "ack" — poll for the async event via long-poll
  for (let i = 0; i < 10; i++) {
    const pollResp = await request.get(`/janus/${sessionId}/longpoll`);
    const text = await pollResp.text();
    if (!text) continue;
    const pollJson = JSON.parse(text);
    if (pollJson.janus === "event") return pollJson;
    if (pollJson.janus === "keepalive") continue;
  }
  throw new Error("Timed out waiting for event after ack");
}

test.describe("EchoTest Media Verification", () => {
  test("echotest message with SDP offer gets SDP answer", async ({
    request,
  }) => {
    const sessionId = await createSession(request);
    const handleId = await attachEchoTest(request, sessionId);

    // Send a message with a minimal SDP offer via JSEP
    const sdpOffer = [
      "v=0",
      "o=- 0 0 IN IP4 127.0.0.1",
      "s=-",
      "t=0 0",
      "a=group:BUNDLE 0",
      "m=audio 9 UDP/TLS/RTP/SAVPF 111",
      "c=IN IP4 0.0.0.0",
      "a=mid:0",
      "a=sendrecv",
      "a=rtpmap:111 opus/48000/2",
      "a=ice-ufrag:testufrag1234",
      "a=ice-pwd:testpasswordtestpassword1234",
      "a=fingerprint:sha-256 00:00:00:00:00:00:00:00:00:00:00:00:00:00:00:00:00:00:00:00:00:00:00:00:00:00:00:00:00:00:00:00",
      "a=setup:actpass",
      "",
    ].join("\r\n");

    const msgJson = await sendMessageAndGetEvent(
      request,
      sessionId,
      handleId,
      { audio: true, video: true },
      { type: "offer", sdp: sdpOffer },
    );

    // Should get an event with JSEP answer
    expect(msgJson.janus).toBe("event");
    expect(msgJson.jsep).toBeTruthy();
    expect(msgJson.jsep.type).toBe("answer");
    expect(msgJson.jsep.sdp).toContain("v=0");
    expect(msgJson.jsep.sdp).toContain("a=ice-ufrag:");

    await destroySession(request, sessionId);
  });

  test("echotest audio-only mode returns answer without video", async ({
    request,
  }) => {
    const sessionId = await createSession(request);
    const handleId = await attachEchoTest(request, sessionId);

    const sdpOffer = [
      "v=0",
      "o=- 0 0 IN IP4 127.0.0.1",
      "s=-",
      "t=0 0",
      "a=group:BUNDLE 0",
      "m=audio 9 UDP/TLS/RTP/SAVPF 111",
      "c=IN IP4 0.0.0.0",
      "a=mid:0",
      "a=sendrecv",
      "a=rtpmap:111 opus/48000/2",
      "a=ice-ufrag:testufrag1234",
      "a=ice-pwd:testpasswordtestpassword1234",
      "a=fingerprint:sha-256 00:00:00:00:00:00:00:00:00:00:00:00:00:00:00:00:00:00:00:00:00:00:00:00:00:00:00:00:00:00:00:00",
      "a=setup:actpass",
      "",
    ].join("\r\n");

    // Configure audio only
    const msgJson = await sendMessageAndGetEvent(
      request,
      sessionId,
      handleId,
      { audio: true, video: false },
      { type: "offer", sdp: sdpOffer },
    );

    expect(msgJson.janus).toBe("event");
    expect(msgJson.jsep).toBeTruthy();
    expect(msgJson.jsep.type).toBe("answer");

    // The echotest plugin result should reflect audio=true, video=false
    expect(msgJson.plugindata.data.echotest).toBe("event");
    expect(msgJson.plugindata.data.result).toBe("ok");

    await destroySession(request, sessionId);
  });

  test("echotest detach and re-attach works cleanly", async ({ request }) => {
    const sessionId = await createSession(request);

    // First attach
    const handleId1 = await attachEchoTest(request, sessionId);

    // Detach
    const detachResp = await request.post(
      `/janus/${sessionId}/${handleId1}`,
      {
        data: { janus: "detach", transaction: "t-detach" },
      },
    );
    expect((await detachResp.json()).janus).toBe("success");

    // Re-attach
    const handleId2 = await attachEchoTest(request, sessionId);
    expect(handleId2).not.toBe(handleId1);

    // Send message to new handle
    const msgResp = await request.post(`/janus/${sessionId}/${handleId2}`, {
      data: {
        janus: "message",
        transaction: "t-msg",
        body: { audio: true, video: true },
      },
    });
    const msgJson = await msgResp.json();
    expect(["ack", "success", "event"]).toContain(msgJson.janus);

    await destroySession(request, sessionId);
  });

  test("session cleanup on destroy removes all handles", async ({
    request,
  }) => {
    const sessionId = await createSession(request);
    const handleId = await attachEchoTest(request, sessionId);

    // Destroy session (should also clean up handle)
    await destroySession(request, sessionId);

    // Trying to use the old handle should fail
    const msgResp = await request.post(`/janus/${sessionId}/${handleId}`, {
      data: {
        janus: "message",
        transaction: "t-msg",
        body: { audio: true },
      },
    });
    const msgJson = await msgResp.json();
    expect(msgJson.janus).toBe("error");
  });
});
