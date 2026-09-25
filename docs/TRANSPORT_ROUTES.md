# Authored transport routes

The supervisor looks for `transports.json` in the bot data directory. Start
from [`transports.example.json`](../transports.example.json). A missing file
keeps transport traversal disabled. A malformed file stops the worker from
starting so a typo cannot silently change route behavior.

Each transport entry is the server gameobject entry. The passenger movement
block reports a runtime transport GUID; code must match that GUID to an
observed gameobject before it can use an authored entry. Stop numbers identify
stops for one transport. Each directed leg needs its own grounded boarding and
exit positions:

```json
{
  "schema_version": 1,
  "transports": [
    {
      "entry": 176495,
      "name": "Example transport",
      "legs": [
        {
          "from_stop": 1,
          "to_stop": 2,
          "boarding": {
            "map": 0,
            "point": { "x": 0.0, "y": 0.0, "z": 0.0 },
            "orientation": 0.0
          },
          "exit": {
            "map": 0,
            "point": { "x": 0.0, "y": 0.0, "z": 0.0 },
            "orientation": 0.0
          }
        }
      ]
    }
  ]
}
```

Replace all example coordinates before use. At worker startup, each point must
project within one yard horizontally and vertically onto a MMAP ground
polygon. Water, unknown surfaces, distant projections, missing navigation
data, and malformed catalog entries are rejected. Rejected legs remain
unavailable and are logged with the transport entry and stop pair.

For a map-aware travel goal, the engine finds a shortest chain of validated
legs from the current map to the goal map. It approaches each authored boarding
point with normal navigation, waits for an authoritative attachment to the
selected transport GUID, waits for authoritative detachment, and then moves to
the authored exit. A missing leg, transport object, attachment, or safe local
step stops traversal and leaves the travel goal waiting.

The catalog does not derive stop positions from client packets or from the
moving transport's position. AzerothCore route data identifies moving
transport paths and map changes; it does not author grounded passenger
boarding and exit points.

Use the [transport catalog generation guide](TRANSPORT_CATALOG_GENERATION.md)
to derive transport entries, stop order, and map topology from AzerothCore data.
