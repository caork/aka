export const AkaPlugin = async ({ client }) => {
  if (client?.app?.log) {
    await client.app.log({
      body: {
        service: "aka",
        level: "info",
        message: "aka OpenCode plugin loaded. Configure the aka MCP server from opencode.json.snippet to enable code graph tools.",
      },
    })
  }

  return {}
}
