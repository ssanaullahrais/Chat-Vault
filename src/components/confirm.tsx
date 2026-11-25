import { useEffect, useState, type ReactNode } from "react";
import { CircleHelp } from "lucide-react";
import { AlertDialog, AlertDialogAction, AlertDialogCancel, AlertDialogContent, AlertDialogDescription, AlertDialogFooter, AlertDialogHeader, AlertDialogTitle } from "@/components/ui/alert-dialog";

export type ConfirmOptions = {
  title: string;
  description: ReactNode;
  /** The words on the button that goes ahead, such as "Sync now". */
  action: string;
  icon?: ReactNode;
  destructive?: boolean;
};

type Pending = ConfirmOptions & { resolve: (ok: boolean) => void };
let show: ((p: Pending) => void) | null = null;

/**
 * Asks before an action runs, in the app's one confirmation box. Resolves true when the user goes ahead.
 * `<ConfirmHost />` must be mounted once (it is, in App).
 */
export function confirmAction(opts: ConfirmOptions): Promise<boolean> {
  return new Promise((resolve) => {
    if (!show) return resolve(true);
    show({ ...opts, resolve });
  });
}

export function ConfirmHost() {
  const [p, setP] = useState<Pending | null>(null);
  useEffect(() => {
    show = (next) => {
      setP((cur) => {
        cur?.resolve(false);
        return next;
      });
    };
    return () => {
      show = null;
    };
  }, []);
  const close = (ok: boolean) => {
    p?.resolve(ok);
    setP(null);
  };
  return (
    <AlertDialog open={!!p} onOpenChange={(o) => !o && close(false)}>
      <AlertDialogContent className="sm:max-w-md">
        <AlertDialogHeader>
          <div className="flex items-start gap-3">
            <div className="flex size-9 shrink-0 items-center justify-center rounded-lg bg-muted">{p?.icon ?? <CircleHelp className="size-4 text-muted-foreground" strokeWidth={1.5} />}</div>
            <div className="min-w-0">
              <AlertDialogTitle className="text-base font-medium">{p?.title}</AlertDialogTitle>
              <AlertDialogDescription>{p?.description}</AlertDialogDescription>
            </div>
          </div>
        </AlertDialogHeader>
        <AlertDialogFooter className="grid grid-cols-2 gap-2 sm:flex">
          <AlertDialogCancel>Cancel</AlertDialogCancel>
          <AlertDialogAction variant={p?.destructive ? "destructive" : "default"} onClick={() => close(true)}>
            {p?.action}
          </AlertDialogAction>
        </AlertDialogFooter>
      </AlertDialogContent>
    </AlertDialog>
  );
}
