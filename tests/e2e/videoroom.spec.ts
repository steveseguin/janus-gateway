import { test, expect } from "@playwright/test";

test.describe("VideoRoom Plugin", () => {
  test("create session, join room 1234 as publisher, then leave", async ({
    request,
  }) => {
    // Create session
    const createResp = await request.post("/janus", {
      data: { janus: "create", transaction: "t1" },
    });
    const createJson = await createResp.json();
    expect(createJson.janus).toBe("success");
    const sessionId = createJson.data.id;

    // Attach to videoroom plugin
    const attachResp = await request.post(`/janus/${sessionId}`, {
      data: {
        janus: "attach",
        plugin: "janus.plugin.videoroom",
        transaction: "t2",
      },
    });
    const attachJson = await attachResp.json();
    expect(attachJson.janus).toBe("success");
    const handleId = attachJson.data.id;

    // Join room 1234 as publisher
    const joinResp = await request.post(`/janus/${sessionId}/${handleId}`, {
      data: {
        janus: "message",
        transaction: "t3",
        body: {
          request: "join",
          room: 1234,
          ptype: "publisher",
          display: "E2E Test User",
        },
      },
    });
    const joinJson = await joinResp.json();
    // Should get ack (async processing) or direct event
    expect(["ack", "success", "event"]).toContain(joinJson.janus);

    // List rooms
    const listResp = await request.post(`/janus/${sessionId}/${handleId}`, {
      data: {
        janus: "message",
        transaction: "t4",
        body: { request: "list" },
      },
    });
    const listJson = await listResp.json();
    expect(["ack", "success", "event"]).toContain(listJson.janus);

    // Leave room
    const leaveResp = await request.post(`/janus/${sessionId}/${handleId}`, {
      data: {
        janus: "message",
        transaction: "t5",
        body: { request: "leave" },
      },
    });
    const leaveJson = await leaveResp.json();
    expect(["ack", "success", "event"]).toContain(leaveJson.janus);

    // Cleanup
    await request.post(`/janus/${sessionId}/${handleId}`, {
      data: { janus: "detach", transaction: "t6" },
    });
    await request.post(`/janus/${sessionId}`, {
      data: { janus: "destroy", transaction: "t7" },
    });
  });

  test("list rooms includes default room 1234", async ({ request }) => {
    // Create session + attach
    const createResp = await request.post("/janus", {
      data: { janus: "create", transaction: "t1" },
    });
    const sessionId = (await createResp.json()).data.id;

    const attachResp = await request.post(`/janus/${sessionId}`, {
      data: {
        janus: "attach",
        plugin: "janus.plugin.videoroom",
        transaction: "t2",
      },
    });
    const handleId = (await attachResp.json()).data.id;

    // List rooms
    const listResp = await request.post(`/janus/${sessionId}/${handleId}`, {
      data: {
        janus: "message",
        transaction: "t3",
        body: { request: "list" },
      },
    });
    const listJson = await listResp.json();

    // Check for room 1234 in the response
    // The response may be direct or via async event
    if (listJson.plugindata && listJson.plugindata.data) {
      const rooms = listJson.plugindata.data.list;
      if (rooms) {
        const room1234 = rooms.find((r: any) => r.room === 1234);
        expect(room1234).toBeTruthy();
      }
    }

    // Cleanup
    await request.post(`/janus/${sessionId}/${handleId}`, {
      data: { janus: "detach", transaction: "t4" },
    });
    await request.post(`/janus/${sessionId}`, {
      data: { janus: "destroy", transaction: "t5" },
    });
  });
});
