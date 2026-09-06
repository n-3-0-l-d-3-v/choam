# Constraints — THE DATABASE

## Primary constraint

The database must run entirely on the project's own immutable storage engine, starting from PUT/GET/DELETE/SCAN/SNAPSHOT before any relational layer exists.

## What it forces

Transactions and MVCC on top of an append-only substrate, real concurrent-client testing, and a constrained SQL subset built bottom-up.

## Research question

How does a relational query engine's design change when its storage layer cannot mutate in place?

## What is explicitly out of scope

See the root [SCOPE.md](../../SCOPE.md) for the CORE / EXTENSION / EXPERIMENT
classification that applies to this repo.
