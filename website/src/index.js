// Serves the static site and sends www.agentscreen.app to the apex.
export default {
  async fetch(request, env) {
    const url = new URL(request.url);
    if (url.hostname === "www.agentscreen.app") {
      url.hostname = "agentscreen.app";
      return Response.redirect(url.toString(), 301);
    }
    return env.ASSETS.fetch(request);
  },
};
