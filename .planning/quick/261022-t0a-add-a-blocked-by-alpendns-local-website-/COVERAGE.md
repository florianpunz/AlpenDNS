# API Coverage — quick task 261022-t0a

No external API integration: this task adds an inbound, unauthenticated HTTP
listener on port 80 that serves one static page, and reads the existing
in-process policy engine for the verdict. It calls no outbound or third-party
API, and no dependency is added. The words "endpoint" and "listener" in the
scope describe our own surface, not an integration.
