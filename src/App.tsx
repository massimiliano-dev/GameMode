import { AnimatePresence, motion } from "motion/react";
import { useEffect, useRef, useState } from "react";

import { Backdrop } from "./components/Backdrop";
import { HintBar, Toasts, TopBar } from "./components/Chrome";
import { LaunchOverlay } from "./components/LaunchOverlay";
import { Priority, useAction } from "./input/dispatcher";
import { ensureFocus, focusElement } from "./input/focus";
import { setupInput } from "./input/sources";
import { GameDetail } from "./screens/GameDetail";
import { Home } from "./screens/Home";
import { Library } from "./screens/Library";
import { QuickMenu } from "./screens/QuickMenu";
import { Settings } from "./screens/Settings";
import { Stores } from "./screens/Stores";
import { currentScreen, type Tab, useApp } from "./store/app";

const TAB_ORDER: Tab[] = ["home", "library", "stores", "settings"];

export default function App() {
  const entry = useApp(currentScreen);
  const depth = useApp((s) => s.stack.length);
  const quickMenu = useApp((s) => s.quickMenu);
  const session = useApp((s) => s.session);
  const [dismissedLaunch, setDismissedLaunch] = useState<string | null>(null);
  const focusBeforeMenu = useRef<HTMLElement | null>(null);

  useEffect(() => {
    void useApp.getState().init();
    void setupInput();
    const onWindowFocus = () => {
      ensureFocus();
      useApp.getState().returnedToApp();
    };
    window.addEventListener("focus", onWindowFocus);
    return () => window.removeEventListener("focus", onWindowFocus);
  }, []);

  useEffect(() => {
    if (quickMenu) focusBeforeMenu.current = document.activeElement as HTMLElement | null;
  }, [quickMenu]);

  const restoreAfterMenu = () => {
    const el = focusBeforeMenu.current;
    if (el?.isConnected) focusElement(el);
    else ensureFocus();
  };

  useAction((action, repeat) => {
    const app = useApp.getState();
    switch (action) {
      case "menu":
      case "guide":
        app.setQuickMenu(true);
        return true;
      case "back":
        if (!repeat) app.back();
        return true;
      case "lb":
      case "rb": {
        if (repeat || app.stack.length > 1) return true;
        const i = TAB_ORDER.indexOf(app.stack[0].screen.name as Tab);
        app.setTab(TAB_ORDER[(i + (action === "rb" ? 1 : TAB_ORDER.length - 1)) % TAB_ORDER.length]);
        return true;
      }
    }
  }, Priority.GLOBAL);

  const launchKey = session ? `${session.gameId}:${session.startedAt}` : null;
  const showLaunch = session?.phase === "launching" && dismissedLaunch !== launchKey;
  const screen = entry.screen;
  const screenKey = `${depth}:${screen.name}:${screen.name === "detail" ? screen.gameId : ""}`;

  return (
    <div className="app" data-nav-layer="root">
      <Backdrop />
      <TopBar />
      <main className="stage">
        <AnimatePresence mode="wait" initial={false}>
          <motion.div
            key={screenKey}
            className="stage-inner"
            initial={{ opacity: 0, y: 14 }}
            animate={{ opacity: 1, y: 0 }}
            exit={{ opacity: 0, y: -6 }}
            transition={{ duration: 0.18, ease: [0.2, 0.8, 0.2, 1] }}
          >
            {screen.name === "home" && <Home focusId={entry.focusId} />}
            {screen.name === "library" && <Library focusId={entry.focusId} />}
            {screen.name === "stores" && <Stores />}
            {screen.name === "settings" && <Settings />}
            {screen.name === "detail" && <GameDetail gameId={screen.gameId} />}
          </motion.div>
        </AnimatePresence>
      </main>
      <HintBar />
      <AnimatePresence onExitComplete={restoreAfterMenu}>{quickMenu && <QuickMenu key="qm" />}</AnimatePresence>
      <AnimatePresence onExitComplete={ensureFocus}>
        {showLaunch && <LaunchOverlay key="launch" onDismiss={() => setDismissedLaunch(launchKey)} />}
      </AnimatePresence>
      <Toasts />
    </div>
  );
}
