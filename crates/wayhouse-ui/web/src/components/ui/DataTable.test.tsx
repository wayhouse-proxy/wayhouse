import { describe, expect, it, vi } from "vitest";
import { render, screen, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import type { ColumnDef } from "@tanstack/react-table";
import { DataTable } from "./DataTable";
import { Pager } from "./Pager";

interface Node { id: string; region: string; conns: number }
const NODES: Node[] = [
  { id: "b-node", region: "eu", conns: 5 },
  { id: "a-node", region: "eu", conns: 20 },
  { id: "c-node", region: "us", conns: 1 },
];
const COLS: ColumnDef<Node>[] = [
  { accessorKey: "id", header: "Node" },
  { accessorKey: "conns", header: "Connections", meta: { numeric: true } },
];
const names = () => screen.getAllByRole("row").slice(1).map((r) => within(r).getAllByRole("cell")[0].textContent);

describe("DataTable", () => {
  it("sorts one column at a time and exposes aria-sort", async () => {
    render(<DataTable columns={COLS} data={NODES} getRowId={(n) => n.id} />);
    await userEvent.click(screen.getByRole("button", { name: "Node" }));
    expect(names()).toEqual(["a-node", "b-node", "c-node"]);
    expect(screen.getByRole("columnheader", { name: "Node" })).toHaveAttribute("aria-sort", "ascending");
    await userEvent.click(screen.getByRole("button", { name: "Connections" }));
    expect(screen.getByRole("columnheader", { name: "Node" })).toHaveAttribute("aria-sort", "none");
    // numbers sort largest first on the first click
    expect(names()).toEqual(["a-node", "b-node", "c-node"]);
    expect(screen.getByRole("columnheader", { name: "Connections" })).toHaveAttribute("aria-sort", "descending");
  });

  it("opens a row on click and on Enter, and marks the selected one", async () => {
    const onOpen = vi.fn();
    render(<DataTable columns={COLS} data={NODES} getRowId={(n) => n.id} onOpen={onOpen} selectedId="a-node" />);
    const row = screen.getByText("a-node").closest("tr")!;
    expect(row).toHaveAttribute("aria-selected", "true");
    row.focus();
    await userEvent.keyboard("{Enter}");
    await userEvent.click(screen.getByText("c-node"));
    expect(onOpen.mock.calls.map((c) => c[0].id)).toEqual(["a-node", "c-node"]);
  });

  it("shows groups with a rollup and collapses them", async () => {
    const groups = [
      { key: "eu", label: "eu", rollup: "2 nodes", rows: NODES.slice(0, 2) },
      { key: "us", label: "us", rollup: "1 node", rows: NODES.slice(2) },
    ];
    render(<DataTable columns={COLS} data={[]} groups={groups} getRowId={(n) => n.id} />);
    expect(screen.getByText("2 nodes")).toBeInTheDocument();
    expect(screen.getByText("b-node")).toBeInTheDocument();
    await userEvent.click(screen.getByRole("button", { name: /^eu/ }));
    expect(screen.queryByText("b-node")).not.toBeInTheDocument();
    expect(screen.getByText("c-node")).toBeInTheDocument();
  });
});

describe("Pager", () => {
  it("says which rows are shown, pages and changes size", async () => {
    const onPage = vi.fn(), onSize = vi.fn();
    render(<Pager page={2} size={25} total={60} onPage={onPage} onSize={onSize} />);
    expect(screen.getByRole("status")).toHaveTextContent("Showing 26 to 50 of 60");
    await userEvent.click(screen.getByRole("button", { name: "Next" }));
    await userEvent.click(screen.getByRole("button", { name: "Previous" }));
    await userEvent.selectOptions(screen.getByLabelText("Per page"), "100");
    expect(onPage.mock.calls).toEqual([[3], [1]]);
    expect(onSize).toHaveBeenCalledWith(100);
  });

  it("disables Next on the last page and shows zero for an empty result", () => {
    const { rerender } = render(<Pager page={3} size={25} total={60} onPage={() => {}} onSize={() => {}} />);
    expect(screen.getByRole("button", { name: "Next" })).toHaveAttribute("aria-disabled", "true");
    rerender(<Pager page={1} size={25} total={0} onPage={() => {}} onSize={() => {}} />);
    expect(screen.getByRole("status")).toHaveTextContent("Showing 0 to 0 of 0");
  });
});
