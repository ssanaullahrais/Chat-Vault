import { useEffect, useMemo, useState, type ReactNode } from "react";
import {
  flexRender,
  getCoreRowModel,
  getPaginationRowModel,
  getSortedRowModel,
  useReactTable,
  type ColumnDef,
  type RowSelectionState,
  type SortingState,
} from "@tanstack/react-table";
import { ArrowDown, ArrowUp, ChevronLeft, ChevronRight, ChevronsUpDown, Inbox } from "lucide-react";
import { Button } from "@/components/ui/button";
import { Checkbox } from "@/components/ui/checkbox";
import { Empty, EmptyDescription, EmptyHeader, EmptyMedia, EmptyTitle } from "@/components/ui/empty";
import { ScrollArea } from "@/components/ui/scroll-area";
import { Skeleton } from "@/components/ui/skeleton";
import { Table, TableBody, TableCell, TableHead, TableHeader, TableRow } from "@/components/ui/table";
import { cn } from "@/lib/utils";

export type Column<T> = {
  id: string;
  header: ReactNode;
  cell: (row: T) => ReactNode;
  /** Makes the header clickable; the value the rows are ordered by. */
  sort?: (row: T) => string | number;
  className?: string;
  headClassName?: string;
};

type Meta = { className?: string; headClassName?: string };

/**
 * The shared table: the shadcn data table (TanStack Table on the shadcn table pieces) with sortable headers, optional
 * row selection and pages. Rows are given in full; sorting and paging happen here, so the caller only filters.
 */
export function DataTable<T>({
  rows,
  columns,
  rowKey,
  pageSize = 10,
  loading = false,
  selectable = false,
  selected,
  onSelectedChange,
  empty,
  initialSort,
  noun = "rows",
  footer = true,
  dense = false,
  minWidth = "min-w-[640px]",
}: {
  rows: T[];
  columns: Column<T>[];
  rowKey: (row: T) => string;
  pageSize?: number;
  loading?: boolean;
  selectable?: boolean;
  selected?: Set<string>;
  onSelectedChange?: (next: Set<string>) => void;
  /** The empty state (shadcn Empty): its icon, title and a line under it. */
  empty?: { icon?: ReactNode; title: string; description?: ReactNode };
  initialSort?: { id: string; desc: boolean };
  /** Used in "1–10 of 240 rows". */
  noun?: string;
  /** The "1–10 of 240" line; with `false` it only shows when there is more than one page. */
  footer?: boolean;
  /** Tighter rows and no header background, for short lists. */
  dense?: boolean;
  /** Below this width the table scrolls sideways (a Tailwind min-width class). */
  minWidth?: string;
}) {
  const [sorting, setSorting] = useState<SortingState>(initialSort ? [initialSort] : []);
  const [pagination, setPagination] = useState({ pageIndex: 0, pageSize });

  const defs = useMemo<ColumnDef<T>[]>(() => {
    const out: ColumnDef<T>[] = columns.map((c) => ({
      id: c.id,
      header: () => c.header,
      cell: ({ row }) => c.cell(row.original),
      enableSorting: !!c.sort,
      accessorFn: c.sort ? (r: T) => c.sort!(r) : undefined,
      sortingFn: (a, b, id) => {
        const x = a.getValue(id) as string | number;
        const y = b.getValue(id) as string | number;
        return typeof x === "number" && typeof y === "number" ? x - y : String(x).localeCompare(String(y), undefined, { numeric: true, sensitivity: "base" });
      },
      sortDescFirst: false,
      meta: { className: c.className, headClassName: c.headClassName } satisfies Meta,
    }));
    if (selectable) {
      out.unshift({
        id: "__select",
        enableSorting: false,
        header: ({ table }) => (
          <Checkbox
            checked={table.getIsAllPageRowsSelected()}
            indeterminate={table.getIsSomePageRowsSelected() && !table.getIsAllPageRowsSelected()}
            onCheckedChange={(v) => table.toggleAllPageRowsSelected(!!v)}
            aria-label="Select every row on this page"
          />
        ),
        cell: ({ row }) => <Checkbox checked={row.getIsSelected()} onCheckedChange={(v) => row.toggleSelected(!!v)} aria-label="Select this row" />,
        meta: { className: "w-9 pr-0", headClassName: "w-9 pr-0" } satisfies Meta,
      });
    }
    return out;
  }, [columns, selectable]);

  // Selection lives with the caller (a Set of row keys), so bulk actions can use it.
  const rowSelection = useMemo<RowSelectionState>(() => Object.fromEntries([...(selected ?? [])].map((k) => [k, true])), [selected]);

  const table = useReactTable({
    data: rows,
    columns: defs,
    getRowId: (r) => rowKey(r),
    state: { sorting, pagination, rowSelection },
    onSortingChange: (u) => {
      setSorting(u);
      setPagination((p) => ({ ...p, pageIndex: 0 }));
    },
    onPaginationChange: setPagination,
    onRowSelectionChange: (u) => {
      const next = typeof u === "function" ? u(rowSelection) : u;
      onSelectedChange?.(new Set(Object.keys(next).filter((k) => next[k])));
    },
    enableRowSelection: selectable,
    enableSortingRemoval: true,
    autoResetPageIndex: false,
    getCoreRowModel: getCoreRowModel(),
    getSortedRowModel: getSortedRowModel(),
    getPaginationRowModel: getPaginationRowModel(),
  });

  const pages = Math.max(1, table.getPageCount());
  // Filtering or deleting can leave fewer pages than before: stay on the last one that exists.
  useEffect(() => {
    if (pagination.pageIndex > pages - 1) setPagination((p) => ({ ...p, pageIndex: pages - 1 }));
  }, [pagination.pageIndex, pages]);

  const total = rows.length;
  const from = total === 0 ? 0 : pagination.pageIndex * pageSize + 1;
  const to = Math.min(total, (pagination.pageIndex + 1) * pageSize);
  const sel = selected?.size ?? 0;
  const cellPad = dense ? "py-1.5" : "";

  return (
    <div className="min-w-0">
      {/* Too narrow for every column: the table scrolls sideways in a shadcn scroll area instead of squeezing. */}
      <ScrollArea horizontal className="rounded-xl border">
        <Table containerClassName="overflow-visible" className={minWidth}>
          <TableHeader className={dense ? "" : "bg-muted/30"}>
            {table.getHeaderGroups().map((g) => (
              <TableRow key={g.id} className="hover:bg-transparent">
                {g.headers.map((h) => {
                  const meta = h.column.columnDef.meta as Meta | undefined;
                  const dir = h.column.getIsSorted();
                  return (
                    <TableHead key={h.id} className={cn(dense && "h-8 text-xs", meta?.headClassName)} aria-sort={dir ? (dir === "desc" ? "descending" : "ascending") : undefined}>
                      {h.column.getCanSort() ? (
                        <button type="button" className="-mx-1 inline-flex cursor-pointer items-center gap-1 rounded px-1 py-0.5 transition-colors hover:text-foreground" onClick={h.column.getToggleSortingHandler()}>
                          {flexRender(h.column.columnDef.header, h.getContext())}
                          {dir === "desc" ? <ArrowDown className="size-3" /> : dir === "asc" ? <ArrowUp className="size-3" /> : <ChevronsUpDown className="size-3 opacity-50" />}
                        </button>
                      ) : (
                        flexRender(h.column.columnDef.header, h.getContext())
                      )}
                    </TableHead>
                  );
                })}
              </TableRow>
            ))}
          </TableHeader>
          <TableBody>
            {loading ? (
              Array.from({ length: Math.min(pageSize, 4) }).map((_, i) => (
                <TableRow key={i} className="hover:bg-transparent">
                  {defs.map((d) => (
                    <TableCell key={d.id} className={cn(cellPad, (d.meta as Meta | undefined)?.className)}>
                      <Skeleton className={d.id === "__select" ? "size-4" : "h-4 w-full max-w-32"} />
                    </TableCell>
                  ))}
                </TableRow>
              ))
            ) : table.getRowModel().rows.length === 0 ? (
              <TableRow className="hover:bg-transparent">
                <TableCell colSpan={defs.length} className="p-0">
                  <Empty className="gap-3 p-6 md:p-8">
                    <EmptyHeader>
                      <EmptyMedia variant="icon">{empty?.icon ?? <Inbox className="size-5" strokeWidth={1.5} />}</EmptyMedia>
                      <EmptyTitle className="text-sm">{empty?.title ?? "Nothing to show"}</EmptyTitle>
                      {empty?.description && <EmptyDescription className="text-xs">{empty.description}</EmptyDescription>}
                    </EmptyHeader>
                  </Empty>
                </TableCell>
              </TableRow>
            ) : (
              table.getRowModel().rows.map((row) => (
                <TableRow key={row.id} data-state={row.getIsSelected() ? "selected" : undefined}>
                  {row.getVisibleCells().map((cell) => (
                    <TableCell key={cell.id} className={cn(cellPad, (cell.column.columnDef.meta as Meta | undefined)?.className)}>
                      {flexRender(cell.column.columnDef.cell, cell.getContext())}
                    </TableCell>
                  ))}
                </TableRow>
              ))
            )}
          </TableBody>
        </Table>
      </ScrollArea>

      {(footer || pages > 1 || sel > 0) && (
        <div className="mt-2.5 flex items-center justify-between gap-3 text-xs text-muted-foreground">
          <span className="min-w-0 truncate tabular-nums">
            {total === 0 ? `No ${noun}` : `${from.toLocaleString()}–${to.toLocaleString()} of ${total.toLocaleString()} ${noun}`}
            {sel > 0 && ` · ${sel.toLocaleString()} selected`}
          </span>
          {pages > 1 && (
            <div className="flex shrink-0 items-center gap-1">
              <Button variant="outline" size="icon" className="size-7" aria-label="Previous page" disabled={!table.getCanPreviousPage()} onClick={() => table.previousPage()}>
                <ChevronLeft className="size-4" />
              </Button>
              <span className="min-w-14 text-center tabular-nums">
                {pagination.pageIndex + 1} / {pages}
              </span>
              <Button variant="outline" size="icon" className="size-7" aria-label="Next page" disabled={!table.getCanNextPage()} onClick={() => table.nextPage()}>
                <ChevronRight className="size-4" />
              </Button>
            </div>
          )}
        </div>
      )}
    </div>
  );
}
