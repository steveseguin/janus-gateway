import { test, expect } from "@playwright/test";

test.describe("Streaming Plugin", () => {
  test("create session and list streaming mountpoints", async ({ request }) => {
    // Create session
    const createResp = await request.post("/janus", {
      data: { janus: "create", transaction: "t1" },
    });
    const createJson = await createResp.json();
    expect(createJson.janus).toBe("success");
    const sessionId = createJson.data.id;

    // Attach to streaming plugin
    const attachResp = await request.post(`/janus/${sessionId}`, {
      data: {
        janus: "attach",
        plugin: "janus.plugin.streaming",
        transaction: "t2",
      },
    });
    const attachJson = await attachResp.json();
    expect(attachJson.janus).toBe("success");
    const handleId = attachJson.data.id;

    // List mountpoints
    const listResp = await request.post(`/janus/${sessionId}/${handleId}`, {
      data: {
        janus: "message",
        transaction: "t3",
        body: { request: "list" },
      },
    });
    const listJson = await listResp.json();
    expect(["ack", "success", "event"]).toContain(listJson.janus);

    // Cleanup
    await request.post(`/janus/${sessionId}/${handleId}`, {
      data: { janus: "detach", transaction: "t4" },
    });
    await request.post(`/janus/${sessionId}`, {
      data: { janus: "destroy", transaction: "t5" },
    });
  });

  test("get info on mountpoint 1", async ({ request }) => {
    // Create session + attach
    const createResp = await request.post("/janus", {
      data: { janus: "create", transaction: "t1" },
    });
    const sessionId = (await createResp.json()).data.id;

    const attachResp = await request.post(`/janus/${sessionId}`, {
      data: {
        janus: "attach",
        plugin: "janus.plugin.streaming",
        transaction: "t2",
      },
    });
    const handleId = (await attachResp.json()).data.id;

    // Get info for mountpoint 1
    const infoResp = await request.post(`/janus/${sessionId}/${handleId}`, {
      data: {
        janus: "message",
        transaction: "t3",
        body: { request: "info", id: 1 },
      },
    });
    const infoJson = await infoResp.json();
    expect(["ack", "success", "event"]).toContain(infoJson.janus);

    // Cleanup
    await request.post(`/janus/${sessionId}/${handleId}`, {
      data: { janus: "detach", transaction: "t4" },
    });
    await request.post(`/janus/${sessionId}`, {
      data: { janus: "destroy", transaction: "t5" },
    });
  });
});
