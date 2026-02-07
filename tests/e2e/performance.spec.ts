import { test, expect } from "@playwright/test";

test.describe("Performance", () => {
  test("50 concurrent sessions created in parallel", async ({ request }) => {
    const concurrency = 50;
    const start = Date.now();

    // Create 50 sessions in parallel
    const createPromises = Array.from({ length: concurrency }, (_, i) =>
      request.post("/janus", {
        data: { janus: "create", transaction: `perf-create-${i}` },
      }),
    );

    const createResponses = await Promise.all(createPromises);
    const createElapsed = Date.now() - start;

    const sessionIds: number[] = [];
    for (const resp of createResponses) {
      const json = await resp.json();
      expect(json.janus).toBe("success");
      sessionIds.push(json.data.id);
    }

    // Verify all IDs are unique
    const uniqueIds = new Set(sessionIds);
    expect(uniqueIds.size).toBe(concurrency);

    // All 50 should complete within 5 seconds
    expect(createElapsed).toBeLessThan(5000);

    // Now attach echotest to all 50 in parallel
    const attachStart = Date.now();
    const attachPromises = sessionIds.map((sid, i) =>
      request.post(`/janus/${sid}`, {
        data: {
          janus: "attach",
          plugin: "janus.plugin.echotest",
          transaction: `perf-attach-${i}`,
        },
      }),
    );

    const attachResponses = await Promise.all(attachPromises);
    const attachElapsed = Date.now() - attachStart;

    for (const resp of attachResponses) {
      const json = await resp.json();
      expect(json.janus).toBe("success");
      expect(json.data.id).toBeTruthy();
    }

    expect(attachElapsed).toBeLessThan(5000);

    // Cleanup: destroy all sessions in parallel
    const destroyPromises = sessionIds.map((sid, i) =>
      request.post(`/janus/${sid}`, {
        data: { janus: "destroy", transaction: `perf-destroy-${i}` },
      }),
    );

    const destroyResponses = await Promise.all(destroyPromises);
    for (const resp of destroyResponses) {
      const json = await resp.json();
      expect(json.janus).toBe("success");
    }

    // Log performance metrics
    console.log(
      `Performance: ${concurrency} sessions created in ${createElapsed}ms, ` +
        `attached in ${attachElapsed}ms`,
    );
  });

  test("rapid message throughput", async ({ request }) => {
    // Create session + attach
    const createResp = await request.post("/janus", {
      data: { janus: "create", transaction: "t1" },
    });
    const sessionId = (await createResp.json()).data.id;

    const attachResp = await request.post(`/janus/${sessionId}`, {
      data: {
        janus: "attach",
        plugin: "janus.plugin.echotest",
        transaction: "t2",
      },
    });
    const handleId = (await attachResp.json()).data.id;

    // Send 100 messages rapidly
    const messageCount = 100;
    const start = Date.now();

    const msgPromises = Array.from({ length: messageCount }, (_, i) =>
      request.post(`/janus/${sessionId}/${handleId}`, {
        data: {
          janus: "message",
          transaction: `msg-${i}`,
          body: { audio: i % 2 === 0, video: true },
        },
      }),
    );

    const msgResponses = await Promise.all(msgPromises);
    const elapsed = Date.now() - start;

    let successCount = 0;
    for (const resp of msgResponses) {
      const json = await resp.json();
      if (json.janus === "event" || json.janus === "ack") {
        successCount++;
      }
    }

    expect(successCount).toBe(messageCount);
    expect(elapsed).toBeLessThan(5000);

    console.log(
      `Throughput: ${messageCount} messages in ${elapsed}ms ` +
        `(${Math.round((messageCount / elapsed) * 1000)} msg/s)`,
    );

    // Cleanup
    await request.post(`/janus/${sessionId}`, {
      data: { janus: "destroy", transaction: "cleanup" },
    });
  });
});
