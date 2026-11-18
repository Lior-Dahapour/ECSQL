# ECSQL: Design Document

A client/server database (like PostgreSQL) built from scratch around the Entity Component System model, written in Rust. Instead of tables that own rows, ECSQL has global entity IDs, typed components, and archetype-grouped columnar storage. Queries are "walk every archetype that has this component set", not joins.

> Status: design draft. Everything here is a starting point for implementation, not a frozen spec. Items marked **[OPEN]** are undecided.

---

## 1. Goals and non-goals

### Goals
- Entity-first data model: an ID is allocated first, and components are attached to it afterwards.
- No joins on ID. Multi-component queries resolve by scanning pre-grouped archetype tables.
- Components are defined at runtime (DDL), not as compile-time Rust types.
- Columnar, cache-friendly storage, in memory and on disk.
- Cheap, first-class "add/remove a component" operation.
- Durable, transactional, crash-safe.
- Client/server, like PostgreSQL: the database runs as its own process and clients connect over the network. Not an embedded library.
- The storage core (`ecsql-core`) is `no_std` + `alloc`, with no OS dependency. Disk, networking, and threads are layered on top.
- Proving ground: persistence for a game server (an MMO backend).

### Non-goals (at least initially)
- Full SQL compatibility. ECSQL is SQL-flavoured, not SQL.
- Distributed operation or replication.
- Ad-hoc many-to-many joins as a primary feature.
- Beating DuckDB at general analytics. The pitch is the entity-and-component model, plus speed on the workloads it fits.

---

## 2. Data model

| Concept | Meaning |
|---|---|
| **Database** | A named container, for example `MYGAMEDB`. Owns one world. |
| **Entity** | A global ID: `(index: u32, generation: u32)`. Owned by no table. |
| **Component type** | A named, typed schema, defined at runtime. Has a numeric `ComponentId`. |
| **Component value** | A fixed-schema record attached to an entity. At most one value per component type per entity. |
| **Archetype** | The unique *set* of component types an entity has. Order does not matter. |
| **Table** | Columnar storage for all entities of one archetype: one packed column per table-stored component, plus an entity-ID column. |
| **Row** | The position within a table. The same row index across all columns is the same entity. |
| **Sparse component** | A component stored outside archetype tables, in a side table keyed by entity ID. Toggling it never moves the entity. |

### Component schemas
A component is a record of typed fields. A **tag** is a component with zero fields.

```sql
CREATE COMPONENT Position (x f32, y f32);
CREATE COMPONENT Health   (current i32, max i32);
CREATE COMPONENT Name     (value text);
CREATE COMPONENT Dead;                          -- tag, no fields
CREATE COMPONENT Stunned  SPARSE;               -- stored in a sparse side table
CREATE COMPONENT Parent   (id entity);          -- relation: holds another entity's ID
```

Initial field types: `i8..i64`, `u8..u64`, `f32`, `f64`, `bool`, `text`, `bytes`, `entity`. **[OPEN]** arrays, optional fields, nested records.

### Entity IDs
- `index` points into the **entity metadata array**. Each slot stores `{ generation, archetype_id, row }`, or a free-list link when unused.
- `generation` increments when a slot is freed, so stale IDs are detected rather than silently matching a reused slot.
- IDs can be allocated in blocks, with no storage touched, so a client can reserve IDs before knowing what the entity will be.

---

## 3. Storage architecture

### 3.1 In-memory core (build this first)

```
World
 ├─ components:  Vec<ComponentInfo>              // ComponentId -> schema, layout, storage kind
 ├─ entities:    EntityMeta[]                    // index -> { gen, archetype, row }
 ├─ archetypes:  Vec<Archetype>
 ├─ arch_index:  HashMap<SortedComponentSet, ArchetypeId>
 └─ sparse:      HashMap<ComponentId, SparseSet>

Archetype
 ├─ id, component_set (sorted Vec<ComponentId>)
 ├─ table: TableId
 ├─ edges: HashMap<ComponentId, { add: ArchetypeId, remove: ArchetypeId }>   // cache
 └─ sparse_components: Vec<ComponentId>

Table
 ├─ entities: Vec<Entity>                          // parallel to every column
 └─ columns:  HashMap<ComponentId, Column>         // Column = tightly packed bytes + stride
```

Rules:
- `arch_index` maps the **sorted** component set to an archetype ID. Spawn or insert looks the set up, reusing the archetype or creating it (with its table).
- Archetypes that differ **only** by sparse components share one table.
- A structural change (insert or remove of a table-stored component) moves the entity to the target archetype by copying its row across tables, then swap-removing the old row and fixing the metadata of the entity that got swapped into the hole.
- The edges cache makes "A plus component X" an O(1) hop to archetype B.
- Columns are runtime-typed byte buffers with a per-component layout (size, alignment, field offsets). **No generics over component types in the hot path.**

### 3.2 Query execution (in memory)

A query is resolved to:
1. `with`: the component set that must be present.
2. `without`: the component set that must be absent.
3. A list of matching archetype IDs, cached per query and **updated incrementally** when a new archetype is created, never by re-scanning every archetype each time.

Execution:
```
for archetype in matching_archetypes:
    table = archetype.table
    for row in 0..table.len():
        read the same row index from each requested column
```
Filters (`WHERE Health.current < 10`) apply per row on the column data. Aggregates (`AVG(Health.current)`) scan only the one column. Both should be vectorized over column slices where possible.

### 3.3 On-disk layout **[OPEN: details]**

Proposed starting design, optimized for simplicity first:

- **Page-based files**, fixed page size (default 8 KiB), each with a header and checksum.
- **Catalog file**: component definitions, archetype definitions (component sets and their table IDs).
- **Table files**: one per table. Column-chunked pages: a page holds a run of rows for one column, so a single-column scan reads only that column's pages.
- **Entity metadata file**: the `index -> { gen, archetype, row }` array, paged.
- **Sparse side tables**: key-ordered pages (B-tree or sorted runs), keyed by entity ID.
- **Write-ahead log (WAL)**: append-only, records every mutation. This is the durability mechanism.
- **Buffer pool**: page cache with eviction. Mmap is tempting but gives up control over write ordering, so prefer an explicit buffer pool. **[OPEN]**

### 3.4 The hard problem: archetype moves on disk

In memory a move is a cheap copy. On disk it is delete-from-one-table plus append-to-another, with crash safety. Strategy, in order of implementation:

1. **WAL first.** A move is logged as one record. The change is durable once the log is flushed.
2. **Lazy physical relocation.** Apply moves to the in-memory/cached representation immediately and write pages back later. A background **compaction** pass rewrites tables and reclaims holes (LSM-like).
3. **Sparse side tables for hot-toggle components** (`Stunned`, `InCombat`, buff flags), so frequent add/remove never moves rows.
4. **Per-transaction batching.** A transaction that adds the same component to N entities becomes one bulk copy per source/target archetype pair, not N single moves.
5. **Coalesce within a transaction.** If one transaction inserts `Inventory` and `SkillBook` on the same entity, compute the final component set and do **one** move to the final archetype, not two hops.

---

## 4. Transactions and recovery

Start simple, harden later.

- **v1:** single writer, many readers. Snapshot isolation via copy-on-write pages or versioned rows **[OPEN: which]**.
- **WAL record types:** `AllocEntity`, `FreeEntity`, `DefineComponent`, `Insert(entity, component, value)`, `Remove(entity, component)`, `Update(entity, component, value)`, `Commit(txn)`.
- **Recovery:** on open, load the last checkpoint, replay committed WAL records after it, discard uncommitted ones.
- **Checkpoints:** periodically flush dirty pages, record the checkpoint LSN, truncate the WAL.
- **Crash testing is mandatory:** fault-injection tests that kill the process at random points in write paths and verify recovery (see section 8).
- **[OPEN]** multiple concurrent writers, and conflict detection granularity (per entity, per archetype). Because ECSQL is a server with many clients, this matters earlier than it would for an embedded database.

---

## 5. Query language (ECSQL)

SQL-flavoured, with the ID-join removed from the syntax. A statement targets a database (`IN MYGAMEDB`). **[OPEN]** exact keyword set and grammar. Treat the examples below as the spec for the first parser.

### 5.1 Entity lifecycle

```sql
-- allocate an ID first (separate step)
e = SPAWN IN MYGAMEDB;

-- attach components to it
INSERT (Inventory(), SkillBook()) FOR e IN MYGAMEDB;

-- one-statement sugar: allocate and insert
SPAWN (Player(), Position(0, 0), Inventory()) IN MYGAMEDB;

-- delete an entity (frees the ID, bumps the generation)
DESPAWN e IN MYGAMEDB;
```

Rules:
- A freshly spawned entity lives in the **empty archetype** until components are inserted.
- Components passed in a single `INSERT` produce a **single** archetype move.
- **[OPEN]** Does `e` bind a variable for the whole session or transaction, or is `FOR` always given a literal ID? Prefer a transaction-scoped binding.

### 5.2 Reads

```sql
SELECT Position, Health FROM entities IN MYGAMEDB;
SELECT Position FROM entities WITHOUT Dead IN MYGAMEDB;
SELECT Name, Health.current FROM entities WHERE Health.current < 10 IN MYGAMEDB;
SELECT AVG(Health.current) FROM entities WITH Position IN MYGAMEDB;
SELECT Name FROM entities WHERE Parent.id = 4182 IN MYGAMEDB;   -- relation lookup
GET e: Position, Health IN MYGAMEDB;                            -- fetch one entity by ID
```

- The set of components named in `SELECT`, `WITH`, and field references in `WHERE` forms the required component set.
- `WITHOUT` excludes archetypes containing the listed components, with no per-row check.
- `entities` is a fixed pseudo-source, not a table name. There is no `JOIN` keyword in v1.

### 5.3 Writes and structural changes

```sql
UPDATE Health SET current = current - 5 WHERE Health.current > 0 IN MYGAMEDB;
ADD Stunned TO entities WHERE Health.current < 10 IN MYGAMEDB;     -- bulk structural change
REMOVE Stunned FROM entities WHERE Stunned.until < NOW() IN MYGAMEDB;
```

### 5.4 DDL

```sql
CREATE DATABASE MYGAMEDB;
CREATE COMPONENT Position (x f32, y f32);
DROP COMPONENT Position;                 -- [OPEN] semantics: remove from all entities, rebuild archetypes
ALTER COMPONENT Health ADD FIELD regen f32 DEFAULT 0;   -- [OPEN]
```

### 5.5 Query planning

The planner is small by design:
1. Resolve component names to `ComponentId`s.
2. Build the `with` and `without` sets.
3. Look up or create a cached query, with its incrementally maintained archetype match list.
4. Choose the access path per matching archetype: full table scan, or sparse-set probe when a sparse component is required.
5. Push filters down into the column scan. Prefer filtering on the most selective column first.

**[OPEN]** indexes (for example a B-tree on `Name.value` or a spatial index on `Position`), kept as per-component optional secondary structures.

---

## 6. Client/server architecture

ECSQL runs as a server process that owns the data. Clients connect over TCP and send statements, like `psql` or a Postgres driver talking to PostgreSQL.

```
client (game server) --TCP--> ecsql-server --> ecsql-lang (parse, plan)
                                            --> ecsql-storage (WAL, pages)
                                            --> ecsql-core (archetypes, queries)
```

### Rust client library

```rust
let client = Client::connect("ecsql://host/MYGAMEDB").await?;

client.execute("CREATE COMPONENT Position (x f32, y f32)").await?;

let mut tx = client.begin().await?;
let e = tx.spawn().await?;                             // allocate ID
tx.insert(e, &[("Inventory", &[]), ("SkillBook", &[])]).await?;
tx.commit().await?;

for row in client.query("SELECT Position, Health FROM entities").await? {
    // row.entity, row.get::<f32>("Position.x"), ...
}
```

- Every call is a network round trip, so set-based statements (`UPDATE ... WHERE`, `ADD ... TO entities WHERE`) matter more than per-entity calls.
- Block ID reservation (section 2) lets a client allocate a batch of entity IDs in one round trip, then spawn locally.
- The server drives the engine through one query IR. The SQL-ish string interface and any typed builder API both compile to it.
- **[OPEN]** wire protocol: a custom binary protocol, or the PostgreSQL wire protocol so existing tools (`psql`, drivers) can connect.
- **[OPEN]** a derive macro for statically typed access from Rust (`#[derive(Component)]`) that registers the schema and gives typed reads without strings. Nice-to-have for the game server.

---

## 7. Project structure (suggested Cargo workspace)

```
ecsql/
 ├─ ecsql-core/      # no_std + alloc: entities, components, archetypes, tables, in-memory queries
 ├─ ecsql-storage/   # pages, buffer pool, WAL, checkpoints, compaction, recovery
 ├─ ecsql-lang/      # lexer, parser, AST, planner, query IR
 ├─ ecsql-server/    # server process: wire protocol, sessions, transactions per connection
 ├─ ecsql-client/    # Rust client library
 ├─ ecsql-cli/       # REPL (in-process engine during early milestones, client later)
 └─ ecsql-bench/     # benchmarks (criterion) and comparison harnesses
```

Crate name: **ecsql**. Check crates.io and GitHub availability first. Fallbacks: `ecsql-db`, `ecsql-core`.

---

## 8. Milestones

1. **M1: In-memory archetype store** (`ecsql-core`)
   - Runtime-defined components, spawn/despawn, insert/remove with archetype moves, edges cache.
   - Query by component set (`with`/`without`) over matching archetypes, with incremental match lists.
   - Unit tests, plus property tests comparing against a naive model (a `HashMap<Entity, HashMap<Component, Value>>`).
2. **M2: Benchmark gate**
   - "AVG(Health) over everything with Position and Health", at 10k / 1M / 10M entities, with varying archetype counts (fragmentation).
   - Compare against DuckDB and, as a baseline, `bevy_ecs`.
   - **Decision point:** if the speed story does not hold, rethink before building the disk layer.
3. **M3: Query language**
   - Lexer, parser, AST, planner for the section 5 examples. REPL.
4. **M4: Persistence**
   - Pages, WAL, checkpoint, recovery. Single writer.
   - Fault-injection crash tests.
5. **M5: Archetype-move optimizations**
   - Per-transaction coalescing and batching, sparse side tables, lazy relocation plus compaction.
6. **M6: Transactions and concurrency**
   - Snapshot isolation, concurrent readers, then (maybe) multiple writers.
7. **M7: Server**
   - `ecsql-server` with a wire protocol, per-connection sessions, and `ecsql-client`.
   - The REPL switches to connecting through the client.
8. **M8: Game-server proving ground**
   - Back an MMO server's persistence with ECSQL. Measure real workloads.

---

## 9. Testing and benchmarking requirements

- **Model-based property tests:** random operation sequences applied to ECSQL and to a trivially correct reference model must yield identical query results. Run on every milestone.
- **Crash/recovery tests:** kill at random points during writes, reopen, and verify committed data is present and uncommitted data is absent.
- **Invariants to assert in debug builds:**
  - `entities[i].archetype/row` always points at a table row whose entity column holds that entity.
  - Every column in a table has the same length as the table's entity column.
  - Archetype component sets are sorted and unique.
- **Benchmarks** (criterion): scan one column, scan many columns, query across many archetypes, bulk add/remove component, spawn throughput, recovery time versus WAL size.

---

## 10. Open questions (collected)

- Final query syntax: keywords, variable binding for `FOR e`, whether to ever support explicit `JOIN`.
- Isolation model and writer concurrency.
- On-disk column encoding: compression (dictionary, RLE, bit-packing) and when to introduce it.
- Archetype explosion: many rarely used component combinations produce many tiny tables. Possible mitigations: merge small tables, cap archetype count, move rare components to sparse storage automatically.
- Secondary indexes and spatial queries.
- Schema evolution (`ALTER COMPONENT`, `DROP COMPONENT`) and its effect on archetypes.
- Buffer pool versus mmap.
- Wire protocol: custom binary, or PostgreSQL wire-protocol compatible.

---

## 11. Notes for the implementer (Claude Code)

- Start with M1 only. Do not build parsing or disk code until the in-memory store passes its property tests.
- Keep component data as untyped byte columns with a runtime layout. Avoid generic-over-component designs in the core.
- Read for ideas, do not copy blindly: `bevy_ecs` (archetype and table storage, edges), `flecs` (runtime-defined components, query language), SQLite (pager, WAL, recovery), DuckDB (columnar execution). Check each project's license before reusing code, not just ideas.
- Prefer small, well-tested steps over large rewrites. After each milestone, record benchmark numbers in `BENCHMARKS.md`.
