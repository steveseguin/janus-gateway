import { test, expect } from "@playwright/test";

/**
 * VideoRoom Multi-Party E2E tests.
 *
 * These tests verify multi-party signaling through the VideoRoom plugin
 * including publisher joins, subscriber notifications, and SDP exchange.
 */

/** Helper: create a Janus session. */
async function createSession(request: any): Promise<number> {
  const resp = await request.post("/janus", {
    data: { janus: "create", transaction: "t-create" },
  });
  const json = await resp.json();
  expect(json.janus).toBe("success");
  return json.data.id;
}

/** Helper: attach to videoroom plugin. */
async function attachVideoRoom(
  request: any,
  sessionId: number,
): Promise<number> {
  const resp = await request.post(`/janus/${sessionId}`, {
    data: {
      janus: "attach",
      plugin: "janus.plugin.videoroom",
      transaction: "t-attach",
    },
  });
  const json = await resp.json();
  expect(json.janus).toBe("success");
  return json.data.id;
}

/** Helper: send a plugin message. */
async function sendMessage(
  request: any,
  sessionId: number,
  handleId: number,
  body: any,
  jsep?: any,
): Promise<any> {
  const data: any = { janus: "message", transaction: "t-msg", body };
  if (jsep) data.jsep = jsep;
  const resp = await request.post(`/janus/${sessionId}/${handleId}`, { data });
  return resp.json();
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
  const data: any = { janus: "message", transaction: `t-${Date.now()}`, body };
  if (jsep) data.jsep = jsep;
  const resp = await request.post(`/janus/${sessionId}/${handleId}`, { data });
  const json = await resp.json();

  if (json.janus === "event" || json.janus === "error") return json;

  // Got "ack" — poll for the async event
  for (let i = 0; i < 20; i++) {
    const pollResp = await request.get(`/janus/${sessionId}?maxev=5`);
    const pollJson = await pollResp.json();
    const events = Array.isArray(pollJson) ? pollJson : [pollJson];
    for (const ev of events) {
      if (ev.janus === "event" || ev.janus === "error") return ev;
    }
    await new Promise((r) => setTimeout(r, 100));
  }
  throw new Error("Timed out waiting for event after ack");
}

/** Helper: poll for events on a session. */
async function pollEvents(request: any, sessionId: number): Promise<any[]> {
  const resp = await request.get(`/janus/${sessionId}?maxev=10`);
  const json = await resp.json();
  if (Array.isArray(json)) return json;
  if (json.janus === "keepalive") return [];
  return [json];
}

const TEST_ROOM = 7777;

test.describe("VideoRoom Multi-Party Signaling", () => {
  test("two publishers join same room and see each other", async ({
    request,
  }) => {
    // Create a room for this test
    const setupSession = await createSession(request);
    const setupHandle = await attachVideoRoom(request, setupSession);
    await sendMessage(request, setupSession, setupHandle, {
      request: "create",
      room: TEST_ROOM,
      max_publishers: 6,
      description: "Multi-party test room",
    });

    // Publisher 1 joins
    const s1 = await createSession(request);
    const h1 = await attachVideoRoom(request, s1);
    const join1 = await sendMessage(request, s1, h1, {
      request: "join",
      room: TEST_ROOM,
      ptype: "publisher",
      display: "Alice",
    });

    expect(join1.plugindata.data.videoroom).toBe("joined");
    expect(join1.plugindata.data.id).toBeTruthy();
    const aliceId = join1.plugindata.data.id;
    // No other publishers yet
    expect(join1.plugindata.data.publishers).toHaveLength(0);

    // Publisher 2 joins
    const s2 = await createSession(request);
    const h2 = await attachVideoRoom(request, s2);
    const join2 = await sendMessage(request, s2, h2, {
      request: "join",
      room: TEST_ROOM,
      ptype: "publisher",
      display: "Bob",
    });

    expect(join2.plugindata.data.videoroom).toBe("joined");
    expect(join2.plugindata.data.id).toBeTruthy();
    const bobId = join2.plugindata.data.id;

    // Publisher 2 should see Publisher 1 in the publishers list
    const publishers = join2.plugindata.data.publishers;
    expect(publishers.length).toBeGreaterThanOrEqual(1);
    const aliceEntry = publishers.find((p: any) => p.id === aliceId);
    expect(aliceEntry).toBeTruthy();
    expect(aliceEntry.display).toBe("Alice");

    // Cleanup
    await destroySession(request, s1);
    await destroySession(request, s2);
    await destroySession(request, setupSession);
  });

  test("publisher with SDP offer gets SDP answer from videoroom", async ({
    request,
  }) => {
    const sessionId = await createSession(request);
    const handleId = await attachVideoRoom(request, sessionId);

    // Join room first
    const joinResp = await sendMessage(request, sessionId, handleId, {
      request: "join",
      room: 1234,
      ptype: "publisher",
      display: "SDPTest",
    });
    expect(joinResp.plugindata.data.videoroom).toBe("joined");

    // Publish with SDP offer
    const sdpOffer = [
      "v=0",
      "o=- 0 0 IN IP4 127.0.0.1",
      "s=-",
      "t=0 0",
      "a=group:BUNDLE 0",
      "m=audio 9 UDP/TLS/RTP/SAVPF 111",
      "c=IN IP4 0.0.0.0",
      "a=mid:0",
      "a=sendonly",
      "a=rtpmap:111 opus/48000/2",
      "a=ice-ufrag:testufrag1234",
      "a=ice-pwd:testpasswordtestpassword1234",
      "a=fingerprint:sha-256 00:00:00:00:00:00:00:00:00:00:00:00:00:00:00:00:00:00:00:00:00:00:00:00:00:00:00:00:00:00:00:00",
      "a=setup:actpass",
      "",
    ].join("\r\n");

    const configResp = await sendMessageAndGetEvent(
      request,
      sessionId,
      handleId,
      { request: "configure", audio: true, video: false },
      { type: "offer", sdp: sdpOffer },
    );

    expect(configResp.janus).toBe("event");
    expect(configResp.jsep).toBeTruthy();
    expect(configResp.jsep.type).toBe("answer");
    expect(configResp.jsep.sdp).toContain("v=0");

    await destroySession(request, sessionId);
  });

  test("subscriber joins and sees publisher list", async ({ request }) => {
    // Create room
    const setupSession = await createSession(request);
    const setupHandle = await attachVideoRoom(request, setupSession);
    await sendMessage(request, setupSession, setupHandle, {
      request: "create",
      room: TEST_ROOM + 1,
      max_publishers: 6,
    });

    // Publisher joins
    const pubSession = await createSession(request);
    const pubHandle = await attachVideoRoom(request, pubSession);
    const joinResp = await sendMessage(request, pubSession, pubHandle, {
      request: "join",
      room: TEST_ROOM + 1,
      ptype: "publisher",
      display: "Publisher1",
    });
    const publisherId = joinResp.plugindata.data.id;

    // Subscriber joins
    const subSession = await createSession(request);
    const subHandle = await attachVideoRoom(request, subSession);
    const subJoin = await sendMessage(request, subSession, subHandle, {
      request: "join",
      room: TEST_ROOM + 1,
      ptype: "subscriber",
      feed: publisherId,
    });

    // Subscriber should get an "attached" response or "event" with offer
    expect(subJoin.janus).toBe("event");
    expect(subJoin.plugindata.data.videoroom).toBe("attached");

    // Cleanup
    await destroySession(request, subSession);
    await destroySession(request, pubSession);
    await destroySession(request, setupSession);
  });

  test("publisher leaving triggers notification", async ({ request }) => {
    // Create room
    const setupSession = await createSession(request);
    const setupHandle = await attachVideoRoom(request, setupSession);
    await sendMessage(request, setupSession, setupHandle, {
      request: "create",
      room: TEST_ROOM + 2,
      max_publishers: 6,
    });

    // Publisher 1 joins
    const s1 = await createSession(request);
    const h1 = await attachVideoRoom(request, s1);
    const join1 = await sendMessage(request, s1, h1, {
      request: "join",
      room: TEST_ROOM + 2,
      ptype: "publisher",
      display: "Leaver",
    });
    const leaverId = join1.plugindata.data.id;

    // Publisher 2 joins
    const s2 = await createSession(request);
    const h2 = await attachVideoRoom(request, s2);
    await sendMessage(request, s2, h2, {
      request: "join",
      room: TEST_ROOM + 2,
      ptype: "publisher",
      display: "Stayer",
    });

    // Publisher 1 leaves (unpublish)
    const leaveResp = await sendMessage(request, s1, h1, {
      request: "leave",
    });
    expect(leaveResp.plugindata.data.videoroom).toBe("event");
    expect(leaveResp.plugindata.data.leaving).toBe("ok");

    // List participants should only show Stayer
    const listResp = await sendMessage(request, s2, h2, {
      request: "listparticipants",
      room: TEST_ROOM + 2,
    });
    const participants = listResp.plugindata?.data?.participants || [];
    const leaver = participants.find((p: any) => p.id === leaverId);
    expect(leaver).toBeFalsy();

    // Cleanup
    await destroySession(request, s1);
    await destroySession(request, s2);
    await destroySession(request, setupSession);
  });

  test("destroy room with active publishers kicks everyone", async ({
    request,
  }) => {
    const setupSession = await createSession(request);
    const setupHandle = await attachVideoRoom(request, setupSession);

    // Create room with secret
    await sendMessage(request, setupSession, setupHandle, {
      request: "create",
      room: TEST_ROOM + 3,
      secret: "roomsecret",
    });

    // Publisher joins
    const pubSession = await createSession(request);
    const pubHandle = await attachVideoRoom(request, pubSession);
    const joinResp = await sendMessage(request, pubSession, pubHandle, {
      request: "join",
      room: TEST_ROOM + 3,
      ptype: "publisher",
      display: "Doomed",
    });
    expect(joinResp.plugindata.data.videoroom).toBe("joined");

    // Destroy room
    const destroyResp = await sendMessage(request, setupSession, setupHandle, {
      request: "destroy",
      room: TEST_ROOM + 3,
      secret: "roomsecret",
    });
    expect(destroyResp.plugindata.data.videoroom).toBe("destroyed");

    // Room should no longer exist
    const existsResp = await sendMessage(request, setupSession, setupHandle, {
      request: "exists",
      room: TEST_ROOM + 3,
    });
    expect(existsResp.plugindata.data.exists).toBe(false);

    // Cleanup
    await destroySession(request, pubSession);
    await destroySession(request, setupSession);
  });
});
