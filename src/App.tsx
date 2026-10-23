import { AnimatePresence, motion } from "motion/react";
import { useEffect, useRef, useState } from "react";

import { Backdrop } from "./components/Backdrop";
import { HintBar, Toasts, TopBar } from "./components/Chrome";
import { LaunchOverlay } from "./components/LaunchOverlay";
import { PowerCountdown } from "./components/PowerCountdown";
import { Screensaver } from "./components/Screensaver";
import { SleepNotice } from "./components/SleepNotice";
import { VirtualKeyboard } from "./components/VirtualKeyboard";
import { Priority, setKeyboardOpener, useAction } from "./input/dispatcher";
import { play } from "./input/sfx";
import { ensureFocus, focusElement } from "./input/focus";
import { setupInput } from "./input/sources";
import { Downloads } from "./screens/Downloads";
import { Connections } from "./screens/Connections";
import { GameDetail } from "./screens/GameDetail";
import { Home } from "./screens/Home";
import { Library } from "./screens/Library";
import { Apps } from "./screens/Apps";
import { QuickMenu } from "./screens/QuickMenu";
import { Search } from "./screens/Search";
import { Settings } from "./screens/Settings";
import { Stats } from "./screens/Stats";
import { Stores } from "./screens/Stores";
import { currentScreen, type Tab, useApp } from "./store/app";
import { useIdle } from "./store/idle";

const TAB_ORDER: Tab[] = ["home", "library", "apps", "downloads", "stores", "stats", "settings"];

export default function App() {
  const entry = useApp(currentScreen);
  const depth = useApp((s) => s.stack.length);
  const quickMenu = useApp((s) => s.quickMenu);
  const session = useApp((s) => s.session);
  const keyboardField = useApp((s) => s.keyboardField);
  const [dismissedLaunch, setDismissedLaunch] = useState<string | null>(null);
  const focusBeforeMenu = useRef<HTMLElement | null>(null);

  useEffect(() => {
    void useApp.getState().init();
    useIdle.getState().init();
    void setupInput();
    setKeyboardOpener((field) => useApp.getState().setKeyboardField(field));
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
        if (!repeat && app.back()) play("back");
        return true;
      case "view":
        if (!repeat && app.stack.at(-1)?.screen.name !== "search") app.open({ name: "search" });
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
            {screen.name === "apps" && <Apps focusId={entry.focusId} />}
            {screen.name === "downloads" && <Downloads />}
            {screen.name === "stores" && <Stores />}
            {screen.name === "stats" && <Stats />}
            {screen.name === "settings" && <Settings />}
            {screen.name === "detail" && <GameDetail gameId={screen.gameId} />}
            {screen.name === "search" && <Search />}
            {screen.name === "connections" && <Connections />}
          </motion.div>
        </AnimatePresence>
      </main>
      <HintBar />
      <AnimatePresence onExitComplete={restoreAfterMenu}>{quickMenu && <QuickMenu key="qm" />}</AnimatePresence>
      <AnimatePresence onExitComplete={ensureFocus}>
        {showLaunch && <LaunchOverlay key="launch" onDismiss={() => setDismissedLaunch(launchKey)} />}
      </AnimatePresence>
      {keyboardField && <VirtualKeyboard field={keyboardField} onClose={() => useApp.getState().setKeyboardField(null)} />}
      <Toasts />
      <PowerCountdown />
      <Screensaver />
      <SleepNotice />
    </div>
  );
}
