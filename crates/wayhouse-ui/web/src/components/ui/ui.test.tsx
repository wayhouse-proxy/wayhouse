import { describe, expect, it, vi } from "vitest";
import { render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { Button } from "./Button";
import { FieldRow } from "./FieldRow";
import { Input, NumberInput, Select } from "./Input";
import { Section } from "./Section";
import { StatusMark } from "./StatusMark";
import { Switch } from "./Switch";
import { Tabs, TabsContent, TabsList, TabsTrigger } from "./Tabs";
import { TH, Table, THead, TR } from "./Table";
import { ToastProvider, useToast } from "./Toast";

describe("Button", () => {
  it("disabled keeps focus, sets aria-disabled and swallows the click", async () => {
    const onClick = vi.fn();
    render(<Button disabled onClick={onClick}>Apply</Button>);
    const b = screen.getByRole("button", { name: "Apply" });
    await userEvent.tab();
    expect(b).toHaveFocus();
    expect(b).toHaveAttribute("aria-disabled", "true");
    await userEvent.click(b);
    expect(onClick).not.toHaveBeenCalled();
  });

  it("loading keeps the label, sets aria-busy and swallows the click", async () => {
    const onClick = vi.fn();
    render(<Button loading onClick={onClick}>Apply</Button>);
    const b = screen.getByRole("button", { name: "Apply" });
    expect(b).toHaveAttribute("aria-busy", "true");
    await userEvent.click(b);
    expect(onClick).not.toHaveBeenCalled();
  });

  it("is a type=button by default so it never submits a form by accident", () => {
    render(<Button>Apply</Button>);
    expect(screen.getByRole("button")).toHaveAttribute("type", "button");
  });
});

describe("FieldRow", () => {
  it("ties label, help and error to the control", () => {
    render(
      <FieldRow label="Idle timeout" help="Closes quiet connections." error="Must be at least 1." changed
        render={(a) => <NumberInput unit="seconds" {...a} />} />,
    );
    const input = screen.getByLabelText(/Idle timeout/);
    expect(input).toHaveAttribute("aria-invalid", "true");
    expect(input).toHaveAccessibleDescription("Closes quiet connections. Must be at least 1.");
    expect(screen.getByText("(changed)")).toBeInTheDocument();
    expect(screen.getByText("seconds")).toBeInTheDocument();
  });

  it("has no aria-invalid without an error", () => {
    render(<FieldRow label="Name" render={(a) => <Input {...a} />} />);
    expect(screen.getByLabelText("Name")).not.toHaveAttribute("aria-invalid");
  });
});

describe("Switch", () => {
  it("is a switch that toggles with the keyboard", async () => {
    const onCheckedChange = vi.fn();
    render(<Switch aria-label="Proxy protocol" onCheckedChange={onCheckedChange} />);
    const s = screen.getByRole("switch", { name: "Proxy protocol" });
    await userEvent.tab();
    await userEvent.keyboard(" ");
    expect(onCheckedChange).toHaveBeenCalledWith(true);
    expect(s).toHaveAttribute("aria-checked", "true");
  });
});

describe("Select", () => {
  it("is a native select", async () => {
    render(<Select aria-label="Region"><option>eu</option><option>us</option></Select>);
    await userEvent.selectOptions(screen.getByRole("combobox", { name: "Region" }), "us");
    expect(screen.getByRole("combobox")).toHaveValue("us");
  });
});

describe("StatusMark", () => {
  it.each([["up", "Up"], ["degraded", "Degraded"], ["down", "Down"], ["unknown", "Unknown"]] as const)(
    "%s always carries its word",
    (state, word) => {
      render(<StatusMark state={state} />);
      expect(screen.getByText(word)).toBeInTheDocument();
    },
  );
});

describe("Section", () => {
  it("is open by default, closed when advanced, and shows the changed count", async () => {
    render(<><Section title="Limits" changed={2}><p>inside</p></Section><Section title="Expert" advanced><p>hidden</p></Section></>);
    expect(screen.getByText("inside")).toBeVisible();
    expect(screen.queryByText("hidden")).not.toBeInTheDocument();
    expect(screen.getByText("2 changed")).toBeInTheDocument();
    await userEvent.click(screen.getByRole("button", { name: "Expert" }));
    expect(screen.getByText("hidden")).toBeVisible();
  });
});

describe("Tabs", () => {
  it("arrow keys move between tabs", async () => {
    render(
      <Tabs defaultValue="a">
        <TabsList><TabsTrigger value="a">Addresses</TabsTrigger><TabsTrigger value="b">Limits</TabsTrigger></TabsList>
        <TabsContent value="a">first</TabsContent><TabsContent value="b">second</TabsContent>
      </Tabs>,
    );
    await userEvent.tab();
    await userEvent.keyboard("{ArrowRight}");
    expect(screen.getByRole("tab", { name: "Limits" })).toHaveAttribute("aria-selected", "true");
    expect(screen.getByText("second")).toBeVisible();
  });
});

describe("Table", () => {
  it("exposes sort state on the header", () => {
    render(<Table><THead><TR><TH sort="ascending">Name</TH></TR></THead></Table>);
    expect(screen.getByRole("columnheader", { name: "Name" })).toHaveAttribute("aria-sort", "ascending");
  });
});

describe("Toast", () => {
  it("announces a finished action", async () => {
    function Go() {
      const toast = useToast();
      return <button onClick={() => toast("Dashboard saved")}>go</button>;
    }
    render(<ToastProvider><Go /></ToastProvider>);
    await userEvent.click(screen.getByText("go"));
    expect(await screen.findByText("Dashboard saved")).toBeInTheDocument();
  });
});
