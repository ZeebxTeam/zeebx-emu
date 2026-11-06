//! IWidget, IControl and IForm: the widget tree and how it is painted.

use super::*;

impl<C: CpuBackend> Machine<C> {
    /// Compatibility with the Z-Wheel intro shipped in package 274755.
    /// It uses the form's own shortcut, without changing instructions or data in the ROM.
    /// The signature keeps the measured addresses from being applied to another applet version.
    pub(super) fn skip_wheel_instructions(&mut self) -> Result<(), CpuError> {
        if self.applet_class != 0x01070798 || self.wheel_boot_skipped {
            return Ok(());
        }
        let Some((function, context)) = self
            .widgets
            .values()
            .map(|widget| widget.handler)
            .find(|(function, _)| *function == 0x11828)
        else {
            return Ok(());
        };
        let mut signature = [0; 16];
        self.cpu.read_mem(function, &mut signature)?;
        if signature
            != [
                0xf0, 0x41, 0x2d, 0xe9, 0x01, 0x0c, 0x51, 0xe3, 0x03, 0x70, 0xa0, 0xe1, 0x01, 0x60,
                0xa0, 0xe1,
            ]
        {
            return Ok(());
        }
        let app = self.cpu.read_u32(context)?;
        // During the transition the form ignores input; wait for the next tick.
        if self.cpu.read_u32(app + 0x2138)? & 1 != 0 {
            return Ok(());
        }
        self.call_guest(
            function,
            [context, input::EVT_KEY, input::avk::CLR, 0],
            QSORT_BUDGET,
        )?;
        self.wheel_boot_skipped = self.cpu.read_u32(app + 0x3610)? & 0x10000000 != 0;
        if self.wheel_boot_skipped {
            self.assumptions.insert("the known Z-Wheel intro automatically gets the skip-instructions shortcut");
        }
        Ok(())
    }

    /// Kicks off the intro, **once** per registered handler.
    ///
    /// From here on `AnimationVideo_Form` runs on its own, and the whole chain has been read:
    /// `0x11528` arms an `ISHELL_SetTimer` of one thousand milliseconds with the callback
    /// `0x114ac`, which is the tick itself; each tick looks at `[form+0x2c]` and advances the
    /// state; in state three `0x11610` registers an `ISHELL_Resume` for `0x11750`, which closes the
    /// form and calls `0x82464` — and that is what leads to the main menu.
    ///
    /// The console gives **one** notice and the rest is up to the game. The notice is the pair
    /// `(0x801, 0x5064)` on the handler that slot 4 registered, which is the shape that `0x11828`
    /// diverts to `0x114ac`.
    ///
    /// Delivering more than one would be inventing a cadence, and that has already cost a hang.
    pub(super) fn start_animation(&mut self) -> Result<(), CpuError> {
        /// The pair `0x11828` understands as "started".
        const START: (u32, u32) = (0x801, 0x5064);

        let fresh: Vec<(u32, (u32, u32))> = self
            .widgets
            .iter()
            .filter(|(_, widget)| widget.handler.0 != 0 && !widget.start_notified)
            .map(|(&object, widget)| (object, widget.handler))
            .collect();
        for (object, (callback, context)) in fresh {
            if let Some(widget) = self.widgets.get_mut(&object) {
                widget.start_notified = true;
            }
            self.call_guest(callback, [context, START.0, START.1, 1], QSORT_BUDGET)?;
        }
        Ok(())
    }

    /// Calls the draw callback that slot 16 registered, one per widget of the current form.
    ///
    /// **Whoever draws an `OwnerDrawWidget` is the game, and whoever tells it to draw is the widget
    /// extension — us.** The widget stores `(function, context)`; the call is
    /// `function(context, screen, x, y)`, with the widget's corner already in screen coordinates.
    ///
    /// The shape of the arguments was read at `0x7562c`, the stage drawing: it saves `r2` and `r3`
    /// and uses them, further on, as the coordinate pair of a stroke. It ignores `r1` — it is the
    /// other `OwnerDrawWidget` of the screen, at `0x605e4`, that does a `QueryInterface` on it. Hence
    /// we pass an `IDisplay`: for the stage it makes no difference, and for the other one the
    /// interface that is missing shows up in the report instead of vanishing.
    ///
    /// Once per frame for each visible widget that registered drawing — and **not** only those of
    /// the current form, unlike what `paint_widgets` does.
    ///
    /// The restriction by tree did exist here and left the stage out: the menu form ends up with
    /// zero children, because it does not receive its content through item `0x5000`, and what it
    /// shows becomes a loose root. Registered drawing is explicit — whoever registered wants to be
    /// called — so it does not depend on us getting right which screen is current. When the
    /// missing link arrives through event `0x7b0a`, it is worth reconsidering.
    pub(super) fn draw_widgets(&mut self) -> Result<(), CpuError> {
        // Draw callbacks are themselves the sign that the widget belongs to the stage. The
        // Z-Wheel keeps the bottom bar in a root separate from the current form; filtering it by
        // the form's tree erases the bar even when it is visible.
        let mut to_call: Vec<(u32, (u32, u32))> = self
            .widgets
            .iter()
            .filter(|(_, node)| node.drawing.0 != 0 && node.visible)
            .map(|(&address, node)| (address, node.drawing))
            .collect();
        // **A covered form does not draw.** With the help screen on top, the stage and the menu
        // wheel — which live in the form below — kept being called, and the stage landed on top of
        // the help panel. Only what belongs to a form that is not the top one is left out; what is
        // under no form at all, like the status bar, keeps being drawn.
        let top = self.current_form();
        to_call.retain(|(address, _)| match self.form_of(*address) {
            Some(form) => Some(form) == top,
            None => true,
        });
        if to_call.is_empty() {
            return Ok(());
        }
        // The order is creation order, as in `paint_widgets`: it is not real depth, but it is
        // stable, and unstable drawing is worse than drawing in the wrong order.
        to_call.sort_by_key(|(address, _)| self.widgets[address].serial);
        let screen = self.target()?;
        for (address, (callback, context)) in to_call {
            let (x, y) = self.screen_position(address);
            self.call_guest(callback, [context, screen, x as u32, y as u32], QSORT_BUDGET)?;
        }
        Ok(())
    }

    /// Marks the form that ended up on top of the root to be told that it is active.
    ///
    /// The notice is `SETPROPERTY(0x5064, 1)` — `FID_ACTIVE` —, the same pair as the intro
    /// start. The menu handler (`0x4e0e8`) reads `0x5064`: with zero, which it receives when
    /// covered, it stops the stage and the wheel (`0x4e3d0`); with one, it gives focus back to the
    /// wheel, restarts the stage and the sound. The Z-Wheel removes the help with
    /// `RemoveForm(root, form)`, and without the notice on the way back the stage stayed still and
    /// the carousel at the top no longer reacted to the arrows.
    ///
    /// **The notice goes out on the next lap of the loop, not here.** When launching a game the
    /// Z-Wheel removes the top form (`0x78a94`) and, in the same call, tears down the rest and
    /// exits. Notifying the menu in the middle of that reactivated it for an instant, and the game
    /// list opened an empty message box instead of launching. See [`Machine::deliver_activation`].
    fn activate_top(&mut self, root: u32) -> Result<(), CpuError> {
        self.pending_activation = self
            .widgets
            .get(&root)
            .and_then(|widget| widget.attached.last().copied())
            .map(|form| (root, form));
        Ok(())
    }

    /// Delivers the notice from [`Machine::activate_top`], if the form is still on top.
    pub(super) fn deliver_activation(&mut self) -> Result<(), CpuError> {
        const SET_PROP: u32 = 0x801;
        const ACTIVE_FORM: u32 = 0x5064;
        let Some((root, form)) = self.pending_activation.take() else {
            return Ok(());
        };
        let still_on_top = self
            .widgets
            .get(&root)
            .is_some_and(|widget| widget.attached.last() == Some(&form));
        let handler = self
            .widgets
            .get(&form)
            .map(|widget| widget.handler)
            .filter(|(callback, _)| *callback != 0);
        if let (true, Some((callback, context))) = (still_on_top, handler) {
            self.call_guest(callback, [context, SET_PROP, ACTIVE_FORM, 1], QSORT_BUDGET)?;
        }
        Ok(())
    }

    /// The form a widget belongs to, walking up through the parents, when there is one.
    fn form_of(&self, widget: u32) -> Option<u32> {
        /// The tree comes from the game; a cycle must not trap the drawing.
        const CAP: usize = 64;
        let mut current = widget;
        for _ in 0..CAP {
            let node = self.widgets.get(&current)?;
            if node.class_id == WIDGET_FORM {
                return Some(current);
            }
            if node.parent == 0 || node.parent == current {
                return None;
            }
            current = node.parent;
        }
        None
    }

    /// Reads the position that `AdicionarFilho` brings in `r3` and stores it in the child.
    ///
    /// The third argument points to six words — `{x, y, flag, width, height, object}`. The game
    /// hangs the intro image with `{0, 0, 1, 640, 480, …}`, the whole screen, and the pieces of
    /// the z-pad form with real coordinates: `{100, 21}`, `{148, 20}`, `{365, 20}`. While this
    /// was ignored, everything was painted at the origin and what was left on screen was a heap in
    /// the corner.
    ///
    /// Width and height come zeroed when the widget measures itself, and then they are not stored:
    /// a zero size would erase what slot 7 already said.
    ///
    /// The same slot serves five classes of the widget family, and not every call has this shape —
    /// some pass a function in `r2` and an object in `r3`. That is why the position is only read
    /// when `r2` is zero, which is the measured shape, and any read that fails is discarded instead
    /// of becoming a coordinate.
    pub(super) fn record_position(&mut self, child: u32) -> Result<(), CpuError> {
        /// Offsets inside the structure, in words.
        const X: u32 = 0;
        const Y: u32 = 4;
        const WIDTH: u32 = 12;
        const HEIGHT: u32 = 16;

        if self.cpu.read_reg(Reg::R2) != 0 {
            return Ok(());
        }
        let location = self.cpu.read_reg(Reg::R3);
        let (Ok(x), Ok(y)) = (self.cpu.read_u32(location + X), self.cpu.read_u32(location + Y)) else {
            return Ok(());
        };
        let size = match (
            self.cpu.read_u32(location + WIDTH),
            self.cpu.read_u32(location + HEIGHT),
        ) {
            (Ok(width), Ok(height)) if width != 0 && height != 0 => Some((width, height)),
            _ => None,
        };
        if let Some(widget) = self.widgets.get_mut(&child) {
            widget.position = (x as i32, y as i32);
            if let Some(size) = size {
                widget.size = size;
            }
        }
        Ok(())
    }

    /// Dumps the widget tree to the serial, one per line, with parent, class and what it carries.
    ///
    /// It exists for studying the widget model: without seeing the whole tree one cannot say
    /// whether it is one or several, and that is exactly what knowing whom a key goes to depends on.
    pub fn dump_widgets(&mut self) {
        if self.serial.is_none() {
            return;
        }
        let mut lines: Vec<(u64, String)> = self
            .widgets
            .iter()
            .map(|(&address, w)| {
                let children = w.children.len() + w.attached.len();
                (
                    w.serial,
                    format!(
                        "<widget {address:#x} parent {:#x} class {:#x} children {children} \
                         handler {:#x} visible {} size {:?} pos {:?} text {:?}>",
                        w.parent, w.class_id, w.handler.0, w.visible, w.size, w.position, w.text
                    ),
                )
            })
            .collect();
        lines.sort_unstable();
        for (_, line) in lines {
            self.log_serial(line);
        }
    }

    /// The handlers that should see a key, saying of each one whether it is on the current screen.
    ///
    /// First those of the current form, from newest to oldest, and then those outside it — where
    /// the intro handler lives, which returns "handled" for any press and, coming first, decided
    /// everything.
    pub(super) fn handlers_in_order(
        &self,
        inside: &std::collections::HashSet<u32>,
    ) -> Vec<(bool, (u32, u32))> {
        let mut marked = Vec::new();
        let mut seen = std::collections::HashSet::new();
        for (&address, node) in &self.widgets {
            if node.handler.0 == 0 || !inside.contains(&address) {
                continue;
            }
            seen.insert(address);
            // A widget in an unfocused branch does not see the key. In the game list the tab bar
            // and the covers live in the same container, and both handle left and right: without
            // looking at the focus, the arrow moved through the covers and switched the tab at the
            // same time.
            if self.out_of_focus(address) {
                continue;
            }
            marked.push((true, node.handler, node.serial));
        }
        marked.sort_by_key(|item| std::cmp::Reverse(item.2));
        let mut outside: Vec<(bool, (u32, u32), u64)> = self
            .widgets
            .iter()
            .filter(|(address, node)| node.handler.0 != 0 && !seen.contains(*address))
            .map(|(_, node)| (false, node.handler, node.serial))
            .collect();
        outside.sort_by_key(|item| std::cmp::Reverse(item.2));
        marked.extend(outside);
        marked.into_iter().map(|(d, pair, _)| (d, pair)).collect()
    }

    /// Whether some container above the widget has a focus set and the focus is not on the path
    /// to it.
    ///
    /// Where no container set a focus, nothing changes: the key keeps going to everyone, which is
    /// the earlier approximation for the screens whose focus does not go through
    /// `EVT_WDG_MOVEFOCUS`.
    fn out_of_focus(&self, widget: u32) -> bool {
        const MOVE_FOCUS: u32 = 0x711;
        /// The tree comes from the game; a cycle in it must not trap key delivery.
        const CAP: usize = 64;
        let mut child = widget;
        for _ in 0..CAP {
            let Some(parent) = self.widgets.get(&child).map(|w| w.parent) else {
                return false;
            };
            if parent == 0 || parent == child {
                return false;
            }
            if let Some(container) = self.widgets.get(&parent) {
                let focus = container.properties.get(&MOVE_FOCUS).copied();
                if let Some(focus) = focus.filter(|f| container.attached.contains(f)) {
                    if focus != child && container.attached.contains(&child) {
                        return true;
                    }
                }
            }
            child = parent;
        }
        false
    }

    /// The widgets of the current form's tree, including the root.
    ///
    /// It walks down from the root instead of up from each widget. Walking up cost one walk per
    /// live widget — and there are twelve thousand of them, because the game rebuilds the form on
    /// every lap and does not release the previous one. The form's tree has a dozen nodes, so
    /// walking down is the cheap way: with the walk up, the loop dropped from 896 laps to 57.
    pub(super) fn form_tree(&self) -> std::collections::HashSet<u32> {
        /// Cap on visited nodes. The tree comes from the game, and a cycle in it must not trap the
        /// drawing.
        const CAP: usize = 4096;

        let mut inside = std::collections::HashSet::new();
        let Some(root) = self.current_form() else {
            return inside;
        };
        let mut queue = vec![root];
        inside.insert(root);
        while let Some(current) = queue.pop() {
            if inside.len() >= CAP {
                break;
            }
            let Some(node) = self.widgets.get(&current) else {
                continue;
            };
            for child in node.children.values().chain(node.attached.iter()) {
                if self.widgets.contains_key(child) && inside.insert(*child) {
                    queue.push(*child);
                }
            }
        }
        // The Z-Wheel bottom bar is not a child of the selected form. It lives in the visual
        // container `0x01028e3f`; including only the newest instance avoids resurrecting all the
        // old trees the shell leaves allocated during attract mode.
        if let Some((&bar, _)) = self
            .widgets
            .iter()
            .filter(|(address, node)| {
                node.class_id == 0x0102_8e3f
                    && (node.parent == 0 || node.parent == **address)
                    && (node.children.len() + node.attached.len()) > 0
            })
            .max_by_key(|(_, node)| node.serial)
        {
            let mut queue = vec![bar];
            inside.insert(bar);
            while let Some(current) = queue.pop() {
                if inside.len() >= CAP {
                    break;
                }
                let Some(node) = self.widgets.get(&current) else {
                    continue;
                };
                for child in node.children.values().chain(node.attached.iter()) {
                    if self.widgets.contains_key(child) && inside.insert(*child) {
                        queue.push(*child);
                    }
                }
            }
        }
        inside
    }

    /// Whether `child` hangs from `container` through slot 5.
    pub(super) fn is_attached_child(&self, container: u32, child: u32) -> bool {
        child != 0
            && self
                .widgets
                .get(&container)
                .is_some_and(|widget| widget.attached.contains(&child))
    }

    /// The newest root of all — the form the game has just built.
    ///
    /// **The Z-Wheel never hides anything.** Slot 6 is only called with true, and the game builds
    /// a new form on every lap of the cycle instead of reusing one: there are 1722 live roots in a
    /// fifteen-second run, each one a whole z-pad form. On the console the previous one would be
    /// destroyed; here it stays, because the game does not release it and we have no reason to
    /// release it on our own.
    ///
    /// Painting all of them stacks the 640×480 welcome screen — which is the **oldest** root — on
    /// top of the current form. The console shows one form at a time, and the current one is the
    /// last built: that is what this function returns.
    pub(super) fn current_form(&self) -> Option<u32> {
        let with_children: std::collections::HashSet<u32> =
            self.widgets.values().map(|node| node.parent).collect();
        // **The newest form.** Ever since item `0x5000` started hanging the content, there is a
        // single tree: the applet root has the forms as children, and each form has the container
        // of what it shows. Painting the whole root draws the intro and the z-pad on top of each
        // other — which is what showed up on screen as soon as the trees were joined.
        //
        // What is on display is the last one stacked, and that is what this function returns.
        // With no form at all — before the interface exists —, the newest root that has a child
        // counts, which is what held the screen up until here.
        let form = self
            .widgets
            .iter()
            .filter(|(address, node)| {
                node.class_id == WIDGET_FORM && with_children.contains(address)
            })
            .max_by_key(|(_, node)| node.serial)
            .map(|(&address, _)| address);
        // **Not every form receives its content through item `0x5000`.** The main menu one does
        // not: it ends up with zero children, and what it shows becomes a loose root — the
        // container of `0x30000c10`, with the status bar and the stage inside. Event `0x7b0a` is
        // already forwarded through the root; the parent link itself has not been observed yet.
        //
        // Meanwhile, choosing only among forms left the z-pad one as current forever, and it
        // brings `0x77300` — a handler that returns "handled" for any key. With it in front, no
        // key reached the menu: the wheel did not turn.
        //
        // So the choice is between the two candidates, and the newest wins.
        let root = self.newest_root(&with_children);
        match (form, root) {
            (Some(f), Some(r)) => {
                let age = |who: u32| self.widgets.get(&who).map_or(0, |node| node.serial);
                return Some(if age(r) > age(f) { r } else { f });
            }
            (Some(f), None) => return Some(f),
            (None, _) => {}
        }
        root
    }

    /// The newest root that has a child, which is what held up the screen before the forms.
    pub(super) fn newest_root(
        &self,
        with_children: &std::collections::HashSet<u32>,
    ) -> Option<u32> {
        self.widgets
            .iter()
            .filter(|(address, node)| {
                // A root is whoever has no parent. **And it must have a child**: the game creates
                // loose widgets that never get to receive anything, and the newest of all is
                // usually one of those — painting from it gave a white screen, which is what showed
                // up.
                (node.parent == 0 || node.parent == **address) && with_children.contains(address)
            })
            .max_by_key(|(_, node)| node.serial)
            .map(|(&address, _)| address)
    }

    /// How deep in the tree a widget is — the root is zero.
    ///
    /// It is what gives the drawing order: child on top of parent. Same cap and same reason as
    /// [`Machine::screen_position`].
    pub(super) fn depth(&self, widget: u32) -> usize {
        /// How far to climb the tree before giving up.
        const BACKGROUND: usize = 32;

        let mut current = widget;
        for height in 0..BACKGROUND {
            let Some(node) = self.widgets.get(&current) else {
                return height;
            };
            if node.parent == 0 || node.parent == current {
                return height;
            }
            current = node.parent;
        }
        BACKGROUND
    }

    /// Where a widget lands on screen, adding up the position of each parent up to the root.
    ///
    /// The loop has a cap because the tree comes from the game: a `parent` that points backwards
    /// would hang the drawing, and a crooked frame is better than a stuck emulator.
    pub(super) fn screen_position(&self, widget: u32) -> (i32, i32) {
        /// How far to climb the tree before giving up.
        const BACKGROUND: usize = 32;

        let (mut x, mut y) = (0, 0);
        let mut current = widget;
        for _ in 0..BACKGROUND {
            let Some(node) = self.widgets.get(&current) else {
                break;
            };
            x += node.position.0;
            y += node.position.1;
            if node.parent == 0 || node.parent == current {
                break;
            }
            current = node.parent;
        }
        (x, y)
    }

    /// Paints the images hung on the widgets.
    ///
    /// **This is a declared stand-in, not an emulation.** On the console whoever draws the
    /// interface is the widget extension, which we don't have: ours keep the tree — children,
    /// size, handler — and rasterize nothing. With nobody painting, the Z-Wheel builds the whole
    /// screen and the frame stays black, which is exactly what was seen.
    ///
    /// What can be done with what is stored is this: every image hung on a widget goes to the
    /// screen. For the Z-Wheel intro that is enough, because it is a 640×480 image at the origin —
    /// the same size the game sends to the widget in slot 7.
    ///
    /// The order is that of the object addresses, which in our allocator is creation order. It is
    /// not real depth; it is the only stable order we have, and a stable order at least makes the
    /// result the same on every run.
    pub(super) fn paint_widgets(&mut self) -> Result<(), CpuError> {
        let current = self.current_form();
        // **A new form erases the previous one.** The surface is persistent: what was painted in
        // one frame is still there in the next. Without clearing, the 640×480 welcome screen stayed
        // on top of everything that came after, even after we stopped drawing it.
        //
        // Clear only on the switch, and not every frame, because whoever draws through `IDisplay`
        // — which is most games — has no form at all and cannot have the screen erased from under
        // it. Without a widget, `current` is `None` and nothing here happens.
        if let Some(root) = current {
            if root != self.painted_form {
                self.painted_form = root;
                let target = self.target()?;
                if let Some(surface) = self.bitmaps.get_mut(&target) {
                    let screen = Rect {
                        x: 0,
                        y: 0,
                        width: surface.width() as i16,
                        height: surface.height() as i16,
                    };
                    surface.fill_rect(screen, Rgb::WHITE);
                }
            }
        }
        let inside = self.form_tree();
        self.paint_backgrounds(&inside)?;
        let mut images: Vec<(u32, u32)> = self
            .widgets
            .iter()
            .filter(|(owner, widget)| widget.visible && inside.contains(*owner))
            .flat_map(|(owner, widget)| widget.attached.iter().map(|&child| (child, *owner)))
            .filter(|(object, _)| self.images.contains_key(object))
            .collect();
        // **Child on top of parent.** The order used to be that of the object addresses, which
        // once was creation order and stopped being so when the allocator gained a free list. With
        // it, the 640×480 background of the welcome screen landed on top of the whole form, and
        // what was left in view was the piece of a widget that happened to have a higher address.
        //
        // The height in the tree is the order the interface means. The address stays as a
        // tie-breaker, so that two runs draw the same.
        images.sort_unstable_by_key(|&(image, owner)| (self.depth(owner), image));
        images.dedup_by_key(|(image, _)| *image);
        for (image, owner) in images {
            let (x, y) = self.screen_position(owner);
            self.draw_image(image, x, y, None)?;
        }
        self.paint_texts()
    }

    /// Fills, every frame, the rectangle of the current screen's widgets that have a background
    /// color.
    ///
    /// The color is [`PROP_BACKGROUND_COLOR`], in `RRGGBBAA` like the text one; zero alpha means
    /// "no background", which is what the Z-Wheel writes on most widgets. The container each of
    /// its forms hangs on `0x5000` gets `0xd0d0d0ff`: it is the light gray behind the stage, the
    /// wheel and the game list. Without it the background stayed at the white of the form switch,
    /// and what a frame left behind — the square of the grid arrow, the little dashes under the
    /// tabs — was never covered.
    ///
    /// A widget with no measured size takes up the whole surface, which is the case of that
    /// container.
    fn paint_backgrounds(&mut self, inside: &std::collections::HashSet<u32>) -> Result<(), CpuError> {
        let mut backgrounds: Vec<(usize, i32, i32, (u32, u32), u32)> = self
            .widgets
            .iter()
            .filter(|(owner, widget)| widget.visible && inside.contains(*owner))
            .filter_map(|(&owner, widget)| {
                let rgba = *widget.properties.get(&PROP_BACKGROUND_COLOR)?;
                (rgba & 0xff != 0).then_some(owner)
            })
            .map(|owner| {
                let (x, y) = self.screen_position(owner);
                let node = &self.widgets[&owner];
                let rgba = node.properties[&PROP_BACKGROUND_COLOR];
                (self.depth(owner), x, y, node.size, rgba)
            })
            .collect();
        if backgrounds.is_empty() {
            return Ok(());
        }
        backgrounds.sort_unstable_by_key(|f| (f.0, f.2, f.1));
        let target = self.target()?;
        let Some(surface) = self.bitmaps.get_mut(&target) else {
            return Ok(());
        };
        for (_, x, y, (fill_width, fill_height), rgba) in backgrounds {
            let (fill_width, fill_height) = if fill_width == 0 || fill_height == 0 {
                (surface.width() as u32, surface.height() as u32)
            } else {
                (fill_width, fill_height)
            };
            let area = Rect {
                x: x as i16,
                y: y as i16,
                width: fill_width.min(i16::MAX as u32) as i16,
                height: fill_height.min(i16::MAX as u32) as i16,
            };
            let color = Rgb {
                r: (rgba >> 24) as u8,
                g: (rgba >> 16) as u8,
                b: (rgba >> 8) as u8,
            };
            surface.fill_rect(area, color);
        }
        Ok(())
    }

    /// Writes the text the widgets store, at their position and in their color.
    ///
    /// It is part of the same stand-in as [`Machine::paint_widgets`]: the console would draw this
    /// through the widget extension, which is not in the dump. What we have is the text — which
    /// arrives through slot 6 of [`WIDGET_TEXT`] — and the font from the game's own package, which
    /// is already loaded.
    ///
    /// The color comes from the [`PROP_COLOR`] property, as `RRGGBBAA`; without it, black. The
    /// alpha is discarded because the console surface has no channel for it — same reason as the
    /// half-tone in [`Machine::write`].
    ///
    /// The order is the tree's, like the images': child on top of parent.
    pub(super) fn paint_texts(&mut self) -> Result<(), CpuError> {
        let inside = self.form_tree();
        let mut writes: Vec<(usize, i32, i32, u32, String)> = self
            .widgets
            .iter()
            .filter(|(owner, widget)| {
                widget.visible && !widget.text.is_empty() && inside.contains(*owner)
            })
            .map(|(&owner, widget)| {
                let (x, y) = self.screen_position(owner);
                let color = widget.properties.get(&PROP_COLOR).copied().unwrap_or(0);
                (self.depth(owner), x, y, color, widget.text.clone())
            })
            .collect();
        // **The same text, in the same place, in the same color, is a single drawing.** The z-pad
        // form is rebuilt on every lap of the cycle and each rebuild leaves the previous tree
        // alive: there are thousands of copies of the same label stacked on the same coordinate.
        // Painting all of them gives exactly the same frame and made a frame take over a minute.
        writes
            .sort_unstable_by(|a, b| (a.1, a.2, &a.4, a.3, a.0).cmp(&(b.1, b.2, &b.4, b.3, b.0)));
        writes.dedup_by(|a, b| (a.1, a.2, &a.4, a.3) == (b.1, b.2, &b.4, b.3));
        // With repeats gone, the order that counts is the tree's: child on top of parent.
        writes.sort_unstable_by(|a, b| (a.0, a.2, a.1).cmp(&(b.0, b.2, b.1)));
        self.paint_html()?;
        for (_, x, y, rgba, text) in writes {
            // The color comes as `RRGGBBAA`. The alpha is discarded because the console surface
            // has no channel for it — same reason as the half-tone in `write`.
            let color = Rgb {
                r: (rgba >> 24) as u8,
                g: (rgba >> 16) as u8,
                b: (rgba >> 8) as u8,
            };
            self.write(&text, x, y, color)?;
        }
        Ok(())
    }

    /// Paints the current screen's HTML widgets with the placeholder text.
    ///
    /// **It is a workaround, not an HTML renderer.** The Z-Wheel help screen needs the firmware's
    /// HTML widget (`0x0102dd32`), which we don't have. Instead of interpreting the page the game
    /// would load, each widget of that class shows the text of [`html_path`], wrapped to its width.
    /// The file lives on the emulated device, outside the ROM, and can be edited freely.
    fn paint_html(&mut self) -> Result<(), CpuError> {
        let inside = self.form_tree();
        let targets: Vec<(u32, i32, i32, u32, u32)> = self
            .widgets
            .iter()
            .filter(|(owner, widget)| {
                widget.class_id == WIDGET_HTML && widget.visible && inside.contains(*owner)
            })
            .map(|(&owner, widget)| {
                let (x, y) = self.screen_position(owner);
                (owner, x, y, widget.size.0, widget.size.1)
            })
            .collect();
        if targets.is_empty() {
            return Ok(());
        }
        for (owner, x, y, width, height) in targets {
            // The page the Z-Wheel delivered, when it delivered one; without it, the placeholder.
            let paragraphs = match self.html_pages.get(&owner) {
                Some(page) => html_text(&decode_page(page)),
                None => html_text(&read_placeholder_html()),
            };
            let width = match width {
                0 => 600,
                l => l.saturating_sub(2 * HTML_MARGIN as u32).max(1),
            };
            let Some(font) = self.font.as_ref() else {
                return Ok(());
            };
            let step = (font.ascent(FONT_SIZE) + font.descent(FONT_SIZE)) as i32 + 2;
            let lines: Vec<String> = paragraphs
                .iter()
                .flat_map(|paragraph| wrap_lines(paragraph, width, |t| font.width(t, FONT_SIZE)))
                .collect();
            let limit = match height {
                0 => i32::MAX,
                a => y + a as i32 - step,
            };
            // How many lines fit, and from that how far scrolling can go.
            let fit_count = match height {
                0 => lines.len(),
                a => ((a as i32 - HTML_MARGIN - step) / step + 1).max(1) as usize,
            };
            let maximum = lines.len().saturating_sub(fit_count);
            let scroll = self.html_scroll.entry(owner).or_insert(0);
            *scroll = (*scroll).min(maximum);
            self.html_max_scroll.insert(owner, maximum);
            let skip = *scroll;
            let mut line_y = y + HTML_MARGIN;
            for line in lines.into_iter().skip(skip) {
                if line_y > limit {
                    break;
                }
                self.write(&line, x + HTML_MARGIN, line_y, Rgb::BLACK)?;
                line_y += step;
            }
        }
        Ok(())
    }

    /// Scrolls the focused HTML widget on the current screen, if there is one and if there is
    /// still somewhere to go.
    ///
    /// On the console whoever scrolls is the firmware's own HTML widget, with the arrows, while it
    /// has focus: the Z-Wheel only draws the panel arrows. Returns whether it scrolled — at the
    /// limit, the key goes on to the handlers, and that is how going up from the top of the text
    /// returns to the tabs.
    pub(super) fn scroll_focused_html(&mut self, downward: bool) -> bool {
        let inside = self.form_tree();
        let Some(target) = self
            .widgets
            .iter()
            .filter(|(address, node)| {
                node.class_id == WIDGET_HTML && node.visible && inside.contains(*address)
            })
            .map(|(&address, _)| address)
            .find(|&address| self.on_focus_path(address))
        else {
            return false;
        };
        let maximum = self.html_max_scroll.get(&target).copied().unwrap_or(0);
        let scroll = self.html_scroll.entry(target).or_insert(0);
        match downward {
            true if *scroll < maximum => *scroll += 1,
            false if *scroll > 0 => *scroll -= 1,
            _ => return false,
        }
        true
    }

    /// Whether some container above the widget set a focus and all those that did point to the
    /// path to it. The opposite of [`Machine::out_of_focus`], but requiring real focus: with no
    /// container with focus at all, the widget is not in focus.
    fn on_focus_path(&self, widget: u32) -> bool {
        const MOVE_FOCUS: u32 = 0x711;
        const CAP: usize = 64;
        // The stored focus is not always a direct child: the help screen stores on its container
        // the HTML widget itself, which is a grandchild. Any node on the path walked so far counts.
        let mut path = vec![widget];
        let mut child = widget;
        let mut focused = false;
        for _ in 0..CAP {
            let Some(parent) = self.widgets.get(&child).map(|w| w.parent) else {
                break;
            };
            if parent == 0 || parent == child {
                break;
            }
            if let Some(container) = self.widgets.get(&parent) {
                match container.properties.get(&MOVE_FOCUS).copied() {
                    Some(focus) if path.contains(&focus) => focused = true,
                    Some(focus) if self.widgets.contains_key(&focus) => return false,
                    _ => {}
                }
            }
            path.push(parent);
            child = parent;
        }
        focused
    }

    /// The child of a widget stored under `id`, created the first time someone asks for it.
    ///
    /// Storing is what makes the accessor consistent with itself: `0x78acc` asks for `0x5000`,
    /// configures it, asks for `0x5002` and releases both at the end. If each request created a
    /// new object, the game would release objects that are not the ones it used.
    pub(super) fn widget_child(&mut self, this: u32, id: u32) -> Result<u32, CpuError> {
        if let Some(child) = self
            .widgets
            .get(&this)
            .and_then(|widget| widget.children.get(&id).copied())
        {
            // Handing out is lending: whoever receives it will release it.
            self.objects.add_ref(child);
            return Ok(child);
        }
        let child = self.new_object(Interface::Widget)?;
        if child == 0 {
            return Ok(0);
        }
        self.next_serial += 1;
        self.widgets.insert(
            child,
            Widget {
                visible: true,
                serial: self.next_serial,
                ..Widget::default()
            },
        );
        if let Some(widget) = self.widgets.get_mut(&this) {
            widget.children.insert(id, child);
        }
        // Two references: **one from the parent's map** and one from whoever asked. Without the
        // parent's, the object died on the caller's first `Release` and the map entry was left
        // pointing at a free address — which, with the allocator reusing addresses, is another
        // object.
        self.objects.add_ref(child);
        Ok(child)
    }

    /// Serves the Z-Wheel widget (`0x01028e51`). See [`Interface::Widget`].
    ///
    /// **The return is inverted**: nonzero is success. The game's wrappers (`0x3f72c` and
    /// `0x403c8`) do `cmp r0,#0; moveq r0,#3`, turning zero into an error. That holds only for
    /// the accessor; `AddRef` and `Release` keep returning the count, as everywhere in BREW.
    ///
    /// The accessor has two selectors, and both were read in the game's code:
    ///
    /// - `0x800` **gets the child** number `id` and writes the pointer to the third argument. The
    ///   child is created the first time and stored: `0x78acc` asks for `0x5000`, configures it,
    ///   and then asks for `0x5002`, and it releases both at the end — if each request created a
    ///   new object, the game would release objects that are not the ones it used.
    /// - `0x801` **sets** property `id`. What the numbers mean we don't know yet; storing them
    ///   costs nothing and is what will let us recognize them when the screen shows up.
    ///
    /// A selector other than these two is refused with zero instead of accepted silently: a third
    /// selector is something we need to see, not hide.
    /// Serves the lightweight control used by the Zenonia text subsystem. The implementation keeps
    /// the ABI and the status answers the game queries, keeping creation from falling into a loop.
    pub(super) fn control_call(&mut self, slot: u32) -> Result<Option<u32>, CpuError> {
        let Some(name) = Interface::Control.method(slot) else {
            return Ok(None);
        };
        let this = self.cpu.read_reg(Reg::R0);
        let result = match name {
            "AddRef" => self.objects.add_ref(this),
            "Release" => self.objects.release(this),
            "IsActive" => 1,
            // Events, activation and reset are operations with no state observable to the emulator.
            "HandleEvent" | "Redraw" | "SetActive" | "SetProperties" | "Reset" => SUCCESS,
            "SetRect" => SUCCESS,
            "GetRect" => {
                let out = self.cpu.read_reg(Reg::R1);
                if out != 0 {
                    self.cpu.write_mem(out, &[0; 16])?;
                }
                SUCCESS
            }
            "GetProperties" => {
                let out = self.cpu.read_reg(Reg::R1);
                if out != 0 {
                    self.cpu.write_u32(out, 0)?;
                }
                SUCCESS
            }
            _ => SUCCESS,
        };
        Ok(Some(result))
    }

    pub(super) fn widget_call(&mut self, slot: u32) -> Result<Option<u32>, CpuError> {
        /// Reads an item of the widget. What comes out depends on the item number — see below.
        const LE: u32 = 0x800;
        /// From here up, the item stores an object; below, a number.
        const FIRST_OBJECT: u32 = 0x5000;
        const SET_PROP: u32 = 0x801;
        /// The HTML widget item that stores an object below the `0x5000` range.
        const PROP_HTML_OBJECT: u32 = 0x161;
        /// Success for this class. It is not BREW's `SUCCESS` — see above.
        const OK: u32 = 1;
        /// Sets the focused child of a container. See the accessor arm.
        const FOCUS_SET: u32 = 0x711;
        /// `WIDGET_FOCUS_PREV`, the largest of the special values of [`FOCUS_SET`].
        const FOCUS_PREVIOUS: u32 = 4;
        /// Reads the focused child set by [`FOCUS_SET`].
        const FOCUS_GET: u32 = 0x713;
        /// Marks the focus state of the widget itself (`id` = 1). See the `0x702` arm.
        const MARK_FOCUS: u32 = 0x700;
        /// Asks, in one byte, whether the widget can take focus.
        const ENABLED: u32 = 0x702;
        /// `FORM_LAST`, the special pointer of `IROOTFORM_RemoveForm` that means "the top one".
        const FORM_LAST: u32 = 1;

        let Some(name) = Interface::Widget.method(slot) else {
            return Ok(None);
        };
        if aee::is_placeholder(name) {
            return Ok(None);
        }
        let this = self.cpu.read_reg(Reg::R0);
        // **Each tab's page reaches the HTML widget through here.** `0x3220c` opens the file,
        // makes an `ISource` of it and hands it over through slot 5 (`0x322e0`) of the object of
        // property `0x161`, which for us is the widget itself. Nobody has read the content yet, and
        // it is what [`Machine::paint_html`] shows in place of the placeholder.
        if name == "AdicionarFilho"
            && self.widgets.get(&this).is_some_and(|w| w.class_id == WIDGET_HTML)
        {
            if let Some(page) = self.sources.get(&self.cpu.read_reg(Reg::R1)) {
                self.html_pages.insert(this, page.clone());
                self.html_scroll.insert(this, 0);
            }
        }
        let result = match name {
            "AddRef" => self.objects.add_ref(this),
            "Release" => self.release_widget(this)?,
            // Zero is success here, unlike the accessor just below. We accept any interface
            // asked for because, in our model, the whole widget family **is** a single interface —
            // the assumption is recorded, which is where it belongs.
            // Slot 12 has the same shape and the same convention as slot 2. It is through it that
            // the Z-Wheel gets, from inside the image callback at `0x4d4a4`, the object on which it
            // will hang the intro GIF: `slot12(IID, &output)` and then `slot5(output, image)`.
            // Refusing it aborted the whole callback, and the animation never started — with no
            // error at all in the log, because whoever aborted was us.
            "QueryInterface" | "PegarInterface" => {
                let output = self.cpu.read_reg(Reg::R2);
                if output != 0 {
                    self.cpu.write_u32(output, this)?;
                }
                self.objects.add_ref(this);
                self.assumptions
                    .insert("a widget accepted every interface it was asked for");
                SUCCESS
            }
            // Slot 6 means two things, depending on the class.
            //
            // In most of the family it is `slot6(this, visible)` — the last thing `0x11750` does
            // before leaving the intro: it hides the animation form and goes to the transition. The
            // return is ignored.
            //
            // In [`WIDGET_TEXT`] it is `slot6(this, text, size, …)`, with the text in `AECHAR`.
            // See the constant for how that was measured.
            // `slot14(this, object)`, with the return ignored. Accepting and storing nothing is the
            // minimum that lets the setup go on; what the object is, we don't know yet.
            // `Slot16(this, triple)` **registers who draws the widget**, and returns the previous
            // one.
            //
            // `CreateOwnerDrawWidget` at `0x22cd0` builds the triple in its own structure before
            // calling: `[r4+0x14] = 0x5250c`, `[r4+0x18] = r4`, `[r4+0x1c] = 0x52574`, and passes
            // `r4+0x14` in `r1`. Whoever draws is `0x5250c`, and it is a link in a chain — it first
            // calls `[ctx+0x14]([ctx+0x18], r1, r2, r3)`, which is exactly where the previous one
            // needs to have been written back, and then the roller's own drawing, in
            // `[ctx+0x00]([ctx+0x08], …)`. Without the hand-back it would call itself.
            //
            // The third word of the triple is `0x52574`, the releaser: stored, and called when the
            // widget dies (see `Release`).
            "Slot13" => {
                // Read at 0x24100..0x24198 of tectoy.mod: (&bitmap, w, h),
                // followed by QueryInterface on the returned bitmap. The widget was used
                // as a bitmap interface through the permissive QueryInterface above.
                let slot = crate::brew::aee_slots::BITMAP
                    .iter()
                    .position(|name| *name == "CreateCompatibleBitmap")
                    .expect("bitmap slot");
                return self.bitmap_call(slot as u32);
            }
            "Slot16" => {
                let location = self.cpu.read_reg(Reg::R1);
                let create = match (self.cpu.read_u32(location), self.cpu.read_u32(location + 4)) {
                    (Ok(callback), Ok(context)) => (callback, context),
                    _ => return Ok(Some(EBADPARM)),
                };
                let free_fn = self.cpu.read_u32(location + 8)?;
                let (previous, previous_releaser) = self
                    .widgets
                    .get(&this)
                    .map_or(((0, 0), 0), |w| (w.drawing, w.releasers.1));
                self.cpu.write_u32(location, previous.0)?;
                self.cpu.write_u32(location + 4, previous.1)?;
                self.cpu.write_u32(location + 8, previous_releaser)?;
                if let Some(widget) = self.widgets.get_mut(&this) {
                    widget.drawing = create;
                    widget.releasers.1 = free_fn;
                }
                if self.serial.is_some() {
                    let own = (self.cpu.read_u32(create.1), self.cpu.read_u32(create.1 + 8));
                    self.log_serial(format!(
                        "<drawing {:#x} ctx {:#x} on {this:#x}; own {own:x?}>",
                        create.0, create.1
                    ));
                }
                SUCCESS
            }
            "Slot17" => {
                // `pWidget->Slot17(0x8000, pFont)` in `tectoy_rollerwidget.c` (0x23860,
                // 0x23d0c). It is not a notification: the module releases `pFont` after building
                // the roller, so the association needs to hold its own reference.
                let (id, model) = (self.cpu.read_reg(Reg::R1), self.cpu.read_reg(Reg::R2));
                if model == 0 || self.objects.kind_of(model).is_none() {
                    return Ok(Some(EBADPARM));
                }
                let Some(previous) = self
                    .widgets
                    .get_mut(&this)
                    .map(|widget| widget.models.insert(id, model))
                else {
                    return Ok(Some(EBADPARM));
                };
                if previous != Some(model) {
                    self.objects.add_ref(model);
                    if let Some(previous) = previous {
                        self.objects.release(previous);
                    }
                }
                SUCCESS
            }
            // `Slot16(this)`, called once at `0x22d58`, right after the stage exists. Refusing it
            // did not return execution to `0x22d5c`: the menu setup stopped there, and the
            // application stayed in the ten-second pulse that reads points and the download queue
            // without drawing anything. Accepted with zero — BREW's `SUCCESS` —, the game goes on:
            // it creates `0x01028e3c`, generates twenty-five textures and uploads nineteen of them
            // compressed in ATITC, builds the frustum and the matrix. It is the stage loading its
            // own scenery.
            //
            // What it does remains unnamed because it was not read: all we know is that it
            // receives the widget and that the game uses the return for nothing but moving on.
            "Anexar" => {
                // Record the link, and not just accept it: without it the tree stayed split in
                // two — the widgets that draw in one half and the key handler in the other —, and
                // any rule about "the current form" picked the wrong half.
                let child = self.cpu.read_reg(Reg::R1);
                if self.widgets.contains_key(&child) {
                    if let Some(widget) = self.widgets.get_mut(&this) {
                        if !widget.attached.contains(&child) && child != this {
                            widget.attached.push(child);
                        }
                    }
                    if let Some(widget) = self.widgets.get_mut(&child) {
                        widget.parent = this;
                    }
                }
                SUCCESS
            }
            // **On containers, slots 6 and 7 are BREW's `IContainer`**: `Remove(widget)` and
            // `GetWidget(pwRef, bNext, bWrap)`, with `Insert` in slot 5, which is already
            // `AdicionarFilho`. The family shares the table, and the two readings are told apart by
            // the arguments: `Remove` receives a child of **this** container, and `GetWidget`
            // receives null or a child followed by two booleans. Neither a size pointer nor a
            // visibility boolean is a child of the container.
            //
            // Without this, `0x80490` — which removes from the root every form except the one about
            // to open, before launching a game — asked for the next child, got garbage from the
            // "size" and ran forever: ten thousand laps until the callback budget ran out, and the
            // game chosen on the grid never opened.
            // **On the root, slot 6 is `IROOTFORM_RemoveForm`**, and `1` is BREW's `FORM_LAST`:
            // `IROOTFORM_PopForm` is exactly `RemoveForm(root, FORM_LAST)`. The Z-Wheel uses it
            // that way twice — at `0x11770`, to remove the intro animation form, and in the loop at
            // `0x82580`, which removes every form before launching a game. Read as "visible", the
            // loop asked for the top, always got the same form and spun until the callback budget
            // ran out.
            "DefinirVisivel"
                if self.widgets.get(&this).map_or(0, |w| w.class_id) == WIDGET_ROOT
                    && self.cpu.read_reg(Reg::R1) == FORM_LAST =>
            {
                // The root releases the reference that `AdicionarFilho` put on the form. Without
                // that the removed screen never died: before launching a game, the Z-Wheel removes
                // every form and tears down its own data, and the list's tab bar stayed alive with
                // the 40 ms animation armed over memory already released. (An old attempt to
                // release here put icons behind the roller; the cause was `ObjectStore`
                // resurrecting free addresses, already fixed.) The `parent` stays.
                let top = self.widgets.get_mut(&this).and_then(|widget| widget.attached.pop());
                if let Some(form) = top {
                    self.release_widget(form)?;
                }
                self.activate_top(this)?;
                SUCCESS
            }
            "DefinirVisivel" if self.is_attached_child(this, self.cpu.read_reg(Reg::R1)) => {
                let child = self.cpu.read_reg(Reg::R1);
                let was_top = self
                    .widgets
                    .get(&this)
                    .is_some_and(|widget| widget.attached.last() == Some(&child));
                if let Some(widget) = self.widgets.get_mut(&this) {
                    widget.attached.retain(|&w| w != child);
                }
                if let Some(child) = self.widgets.get_mut(&child) {
                    if child.parent == this {
                        child.parent = 0;
                    }
                }
                self.release_widget(child)?;
                // Only when the top form left, like BREW's `RootForm`: before launching a game the
                // Z-Wheel removes the lower forms from the root, and reactivating the menu for each
                // of them made the game list open an empty message box instead of launching.
                if was_top && self.widgets.get(&this).map_or(0, |w| w.class_id) == WIDGET_ROOT {
                    self.activate_top(this)?;
                }
                SUCCESS
            }
            "DefinirTamanho"
                if {
                    let (reference, next, tick) = (
                        self.cpu.read_reg(Reg::R1),
                        self.cpu.read_reg(Reg::R2),
                        self.cpu.read_reg(Reg::R3),
                    );
                    (reference == 0 || self.is_attached_child(this, reference))
                        && next <= 1
                        && tick <= 1
                } =>
            {
                let (reference, next, tick) = (
                    self.cpu.read_reg(Reg::R1),
                    self.cpu.read_reg(Reg::R2) != 0,
                    self.cpu.read_reg(Reg::R3) != 0,
                );
                let children = self
                    .widgets
                    .get(&this)
                    .map(|widget| widget.attached.clone())
                    .unwrap_or_default();
                // The stack goes from the first inserted to the last. With no reference, "next"
                // starts from the bottom and "previous" from the top. `GetWidget` gives no
                // reference to the receiver: it is a borrowed pointer.
                stack_neighbor(&children, reference, next, tick)
            }
            "DefinirVisivel" => {
                let class_id = self.widgets.get(&this).map_or(0, |widget| widget.class_id);
                if class_id == WIDGET_TEXT {
                    let text = self.read_aechar(self.cpu.read_reg(Reg::R1))?;
                    if let Some(widget) = self.widgets.get_mut(&this) {
                        widget.text = text;
                    }
                    return Ok(Some(SUCCESS));
                }
                let visible = self.cpu.read_reg(Reg::R1) != 0;
                if let Some(widget) = self.widgets.get_mut(&this) {
                    widget.visible = visible;
                }
                SUCCESS
            }
            // `slot8(this, &output)`, called on every intro tick by `0x11578`. What comes out of
            // it then receives a `slot3(widget, 0, 0)` and is released — and slot 3 on a widget is
            // the accessor. A widget pointer in place of the selector is no selector at all, so the
            // reading that remains is: **the parent is whoever gets the notice that the child
            // changed**, and the accessor refuses what it does not know, which is what it already
            // does.
            //
            // **Unconfirmed reading.** While nobody kicks off the animation, this slot is not
            // called, so there is no run to support or overturn the hypothesis. It stays written
            // down for the next one who passes by here.
            //
            // Return it with a count, because whoever receives it releases it right after.
            "PegarPai" => {
                let output = self.cpu.read_reg(Reg::R1);
                let parent = self.widgets.get(&this).map_or(0, |widget| widget.parent);
                if parent != 0 {
                    self.objects.add_ref(parent);
                }
                if output != 0 {
                    self.cpu.write_u32(output, parent)?;
                }
                SUCCESS
            }
            // `slot4(this, &handler)`, seen at `0x11a6c`. What `r1` points to is built just above,
            // at `0x11a60`: the game object puts itself as context at `+0x1c` and the function
            // address at `+0x20`. It is the registration of an event handler.
            //
            // We store the address and call nobody: whoever would fire these events is the
            // interface we don't draw yet. When it exists, the handler is here.
            // `slot4(this, &{function, context, free_fn})`.
            //
            // **It returns the previous handler**, writing it back into the structure it received.
            // That is how BREW chains: whoever registers keeps there whoever was before and
            // diverts to it what it does not handle. `ZPad_Keyboard_Instructions_Form.c` does
            // exactly that — `0x8f560` reads `[context+0x14]` and does a tail jump there when it
            // does not handle the key.
            //
            // Without writing anything back, the structure kept describing **the very** handler
            // that had just registered, and the diversion became infinite recursion: 250 million
            // instructions in a single frame, the loop dropping from 897 laps to two and the window
            // freezing. There was no error in the report because there was no error — there was a
            // loop.
            //
            // With no previous handler, what comes back is zero, and `0x8f564` recognizes that.
            "DefinirTratador" => {
                let location = self.cpu.read_reg(Reg::R1);
                let create = match (self.cpu.read_u32(location), self.cpu.read_u32(location + 4)) {
                    (Ok(callback), Ok(context)) => (callback, context),
                    _ => return Ok(Some(EBADPARM)),
                };
                let free_fn = self.cpu.read_u32(location + 8)?;
                let (previous, previous_releaser) = self
                    .widgets
                    .get(&this)
                    .map_or(((0, 0), 0), |w| (w.handler, w.releasers.0));
                self.cpu.write_u32(location, previous.0)?;
                self.cpu.write_u32(location + 4, previous.1)?;
                self.cpu.write_u32(location + 8, previous_releaser)?;
                if let Some(widget) = self.widgets.get_mut(&this) {
                    widget.handler = create;
                    widget.releasers.0 = free_fn;
                }
                SUCCESS
            }
            // `slot5(this, child, 0, &position, …)`, seen at `0x11cc0`: the game hangs one widget
            // on the other and releases its reference right after. The return is ignored — the
            // next instruction already overwrites `r0`.
            //
            // Here the child is only recorded. There is no interface tree to build while nobody
            // draws through it, and storing the link is what lets us recognize, when that changes,
            // that it already existed.
            // Slot 5 means two things, and the **third argument** tells them apart.
            //
            // With `r2 == 0` it is `slot5(this, child, 0, &position, …)`: hangs one widget on the
            // other.
            //
            // With `r2 == 4` it is a **getter**: `slot5(this, &output, 4)`, and the output is two
            // half-words. `0x8fa04` makes that call and then adds the two (`0x8fa20`), divides by
            // them at `0x10a18` and multiplies back — it is the step of a list, and the calculation
            // `(height - 20) / step * step + 10` says how many items fit. Treating everything as
            // "hang a child", nothing was written and the sum was zero: a division by zero, which
            // the game's runtime prints through semihosting as
            // `Arithmetic exception: Divide By Zero`.
            //
            // The step we return is the font's line height, which is what we have. It is an
            // assumption, and it is recorded: a wrong step gets the layout wrong, a zero step
            // crashes.
            "AdicionarFilho" if self.cpu.read_reg(Reg::R2) == 4 => {
                /// Two half-words, added up by the caller.
                const STEP: i16 = FONT_SIZE as i16;

                let output = self.cpu.read_reg(Reg::R1);
                if output != 0 {
                    self.cpu.write_mem(output, &STEP.to_le_bytes())?;
                    self.cpu.write_mem(output + 2, &0i16.to_le_bytes())?;
                }
                self.assumptions.insert(
                    "the list step that slot 5 asked for was answered with the font height",
                );
                SUCCESS
            }
            "AdicionarFilho"
                if self
                    .widgets
                    .get(&this)
                    .is_some_and(|w| matches!(w.class_id, WIDGET_TEXT | 0x01028e19))
                    && !self.widgets.contains_key(&self.cpu.read_reg(Reg::R1))
                    && !self.images.contains_key(&self.cpu.read_reg(Reg::R1)) =>
            {
                // IWidget::GetExtent(&{cx, cy}) on the text/image interfaces.
                // The status bar uses the label's cx to position the credits:
                // at 0x857ec it reads cx and at 0x85828 it adds 375. Leaving the output zeroed
                // put "10" on top of "Meus Z-Credits".
                let widget = &self.widgets[&this];
                let size = if widget.class_id == WIDGET_TEXT {
                    self.font
                        .as_ref()
                        .map(|font| {
                            (
                                font.width(&widget.text, FONT_SIZE),
                                font.ascent(FONT_SIZE) + font.descent(FONT_SIZE),
                            )
                        })
                        .unwrap_or((0, 0))
                } else {
                    widget
                        .attached
                        .iter()
                        .find_map(|id| self.images.get(id))
                        .map(|image| (image.width, image.height))
                        .unwrap_or(widget.size)
                };
                let out = self.cpu.read_reg(Reg::R1);
                if out == 0 {
                    return Ok(Some(EBADPARM));
                }
                self.cpu.write_u32(out, size.0)?;
                self.cpu.write_u32(out + 4, size.1)?;
                SUCCESS
            }
            "AdicionarFilho" => {
                let child = self.cpu.read_reg(Reg::R1);
                self.record_position(child)?;
                let already_attached = self
                    .widgets
                    .get(&this)
                    .is_some_and(|widget| widget.attached.contains(&child));
                if let Some(widget) = self.widgets.get_mut(&this) {
                    if child != this && !already_attached {
                        widget.attached.push(child);
                    }
                }
                if let Some(child) = self.widgets.get_mut(&child) {
                    child.parent = this;
                }
                // Whoever stores, holds. It is the convention of all of BREW, and here it is not
                // theory: right after hanging the image on the widget, the Z-Wheel releases its
                // reference. Without this count, the object died with the decoded image inside —
                // and what was left to paint was nothing.
                if !already_attached && child != this {
                    self.objects.add_ref(child);
                }
                SUCCESS
            }
            // `slot7(this, &{width, height})`, seen at `0x11c90` with `640 × 480` — the whole
            // screen. The game ignores the return: the next instruction already overwrites `r0`.
            "DefinirTamanho" => {
                // The pointer is not always a pointer. When the widget tree gets large, the game
                // calls this slot with `r1` pointing outside the map, and reading from there brings
                // down the ARM core — it showed up as `READ_UNMAPPED` in the interface report.
                // Ignoring what cannot be read is right: a size that did not come is a size that
                // does not change.
                let pair = self.cpu.read_reg(Reg::R1);
                let new_size = match (self.cpu.read_u32(pair), self.cpu.read_u32(pair + 4)) {
                    (Ok(width), Ok(height)) => (width, height),
                    _ => return Ok(Some(EBADPARM)),
                };
                if let Some(widget) = self.widgets.get_mut(&this) {
                    widget.size = new_size;
                }
                SUCCESS
            }
            "Acessador" => {
                let (selector, id, third) = (
                    self.cpu.read_reg(Reg::R1),
                    self.cpu.read_reg(Reg::R2),
                    self.cpu.read_reg(Reg::R3),
                );
                // **What each widget class receives, counted per selector.** It is the instrument
                // to answer "what behavior does this proprietary class expect?" without a probe and
                // without disassembly: the count per `(class_id, selector)` pair says what the game
                // actually uses, and not what the whole family could use. `0x01028e19` is the case
                // that motivated it: it joins the widget family by numbering proximity, and what is
                // known about it is what was measured.
                //
                // **Off by default**, and the reason is measured: the census goes into the report,
                // the report goes into the baseline, and a new section would make the scan's
                // **62 games** flag a difference all at once. It is turned on with
                // `ZEEBX_ROM_SELECTORS`, like the cost profile.
                if self.widget_census {
                    let class_id = self.widgets.get(&this).map_or(0, |widget| widget.class_id);
                    let times = {
                        let count = self.selectors_per_class.entry((class_id, selector)).or_insert(0);
                        *count += 1;
                        *count
                    };
                    // **The capture is the report of whoever has no report.** The scan prints the
                    // census in its report; the core has nowhere to print it, and the serial
                    // capture is where it already writes everything else. Without this line, the
                    // census turned on in the core would stay only in the process memory.
                    if self.serial.is_some() {
                        self.log_serial(format!(
                            "<accessor class {class_id:#010x} selector {selector:#x} ({times}x)>"
                        ));
                    }
                }
                match selector {
                    // Some classes use a child's own address as the selector to query/toggle the
                    // visual state. It is an operation with no return value; recognizing the
                    // address keeps the tree moving and avoids treating it as an unknown property
                    // code.
                    _ if self.widgets.contains_key(&selector) => OK,
                    // Events offered first to the root by the applet (0x7b418).
                    // A true return interrupts the dispatch at 0x7b420, before the applet's own
                    // handlers that fill in the font/resource outputs.
                    // With no handler here, returning false lets the applet resolve them.
                    0x101 | 0x7b0a | 0x7b0e | 0x7b0f => FALSE,
                    LE => {
                        // **Not every `0x800` asks for a child.** The game reads and writes things
                        // of different types through the same selector, and the item number is what
                        // says which: those in the `0x5000` range store **objects** — `0x88338`
                        // reads `0x5000` and `0x5002` and uses both as widgets —, and the ones below
                        // are **numbers**.
                        //
                        // This is not deduction: the image callback reads item `0x347`, adds two
                        // and writes it back. While the read created a child for any item, what it
                        // added two to was **one of our pointers**, and what it wrote to
                        // `[form+0x24]` through item `0x414` was another one. A pointer where the
                        // game expects a number is the start of a sequence of symptoms that do not
                        // look like the cause.
                        // The widget items are **typed**, and the measured cut is `0x5000`: from
                        // there up the item stores an object, below it stores a number.
                        //
                        // I did put `0x414` in as an exception, thinking it stored a widget: the
                        // image callback reads it into `[form+0x24]`, and `0x11750` does
                        // `[r0+0x24]->slot6(1)`. They were **different** fields — `0x11750`
                        // receives the **application**, not the form, and the object it releases
                        // is another one. What `0x414` stores really is a number, and `0x11668`
                        // proves it: it subtracts one and compares.
                        //
                        // The exception cost dearly while it lasted. With a pointer there, the
                        // comparison at `0x11674` was never true and the intro spun forever:
                        // twelve million trips to the accessor and six million timers in a single
                        // run.
                        // On the HTML widget, `0x161` is an object too: the help screen
                        // (`0x31c34`) reads the item and does a `QueryInterface(0x102d691)` on it
                        // without checking for zero. With a number there the setup aborted
                        // silently, and what was already created was left loose on screen, with no
                        // form.
                        let object = id >= FIRST_OBJECT
                            || (id == PROP_HTML_OBJECT
                                && self.widgets.get(&this).is_some_and(|w| w.class_id == WIDGET_HTML));
                        let value = match object {
                            true => self.widget_child(this, id)?,
                            false => self
                                .widgets
                                .get(&this)
                                .and_then(|widget| widget.properties.get(&id).copied())
                                .unwrap_or(0),
                        };
                        if third != 0 {
                            self.cpu.write_u32(third, value)?;
                        }
                        OK
                    }
                    SET_PROP => {
                        // The properties go to the serial, which is where the instrumentation lives.
                        if self.serial.is_some() {
                            self.log_serial(format!(
                                "<prop {id:#x}={third:#x} on {this:#x}>"
                            ));
                        }
                        // **Writing an object to an item in the `0x5000` range is hanging it.** It
                        // is through here that the form receives what it shows: `0x8ed80` writes
                        // `0x5000` on the z-pad form with the container of the status bar and of
                        // the controller photo, and the intro does the same with its own.
                        //
                        // Without treating this as a link, the tree stayed split in three — the
                        // forms in one, what gets drawn in another — and no rule about "the current
                        // screen" could get it right, because the key and the drawing lived in
                        // different trees. We kept the number and lost the parentage.
                        if id >= FIRST_OBJECT && self.widgets.contains_key(&third) {
                            if let Some(widget) = self.widgets.get_mut(&this) {
                                widget.children.insert(id, third);
                            }
                            if let Some(child) = self.widgets.get_mut(&third) {
                                child.parent = this;
                            }
                        }
                        if let Some(widget) = self.widgets.get_mut(&this) {
                            widget.properties.insert(id, third);
                        }
                        OK
                    }
                    // **`0x711` sets the focused child of a container, and `0x713` reads it.**
                    //
                    // The pair came from usage, not from a header. Through `0x711` the Z-Wheel
                    // writes a widget (`0x30000d90`) on the roller's container, and it is **on that
                    // same container** that it asks for `0x713` when the player confirms — through
                    // a wrapper at `0x3fe5c` that passes `&output`. At `0x4eb5c` it compares the
                    // answer with the widget of the item it thinks is selected (`[[r4+0x24]+0xb0]`)
                    // and only runs the action if the two match. While `0x713` was refused,
                    // confirming did nothing — neither "Jogar" nor "Ajuda".
                    //
                    // `0x711` is `EVT_WDG_MOVEFOCUS`, and the argument may not be a widget: the
                    // values `1` to `4` are `WIDGET_FOCUS_FIRST`, `LAST`, `NEXT` and `PREV`, and
                    // `0` is `NONE`. It is with `NEXT` and `PREV` that the game list moves the
                    // focus from the tab bar to the covers (`0x4025c`, called with `3`); storing the
                    // `3` as if it were the focused child left the focus nowhere and nothing was
                    // highlighted.
                    FOCUS_SET if !self.widgets.contains_key(&third) && third <= FOCUS_PREVIOUS => {
                        u32::from(self.move_focus(this, third)?)
                    }
                    // With a real widget, whoever leaves and whoever enters are also notified. That
                    // is how the main menu gives focus to the wheel (`MOVEFOCUS(run)` at 7 s), and it
                    // is the notice that turns on the blue frame of the chosen item.
                    FOCUS_SET => {
                        let previous = self
                            .widgets
                            .get(&this)
                            .and_then(|widget| widget.properties.get(&FOCUS_SET).copied())
                            .unwrap_or(0);
                        if let Some(widget) = self.widgets.get_mut(&this) {
                            widget.properties.insert(FOCUS_SET, third);
                        }
                        if previous != third {
                            if self.widgets.contains_key(&previous) {
                                self.notify_widget(previous, MARK_FOCUS, 0)?;
                            }
                            self.notify_widget(third, MARK_FOCUS, 1)?;
                        }
                        OK
                    }
                    // `0x702` answers **one byte** — the caller reads it with `ldrb` — and both uses
                    // treat it as "can": at `0x394b4` the item's action only runs if it is true,
                    // and at `0x399a0` the widget entering focus only receives `0x700`
                    // (`accessor(widget, 0x700, 1, 0)`) if it is true. The reading is "enabled for
                    // focus", and every widget of ours is enabled; `0x700` is stored for when
                    // something reads it.
                    ENABLED => {
                        if third != 0 {
                            self.cpu.write_mem(third, &[1])?;
                        }
                        self.assumptions.insert(
                            "widget selector 0x702 answered enabled — the reading comes from usage, no header",
                        );
                        OK
                    }
                    // Marking focus on a widget is what gives it focus **inside the parent**. The
                    // Z-Wheel game grid never writes `0x711` on its container: when choosing an
                    // item, it calls `accessor(item, 0x700, 1, 0)` on the item's widget (`0x399d0`)
                    // and then, on confirming, asks the item's **parent** for `0x713`. Without
                    // carrying the focus to the parent, the question came back null and confirming
                    // did not open the game — measured: item `0x30001250` has parent `0x30000f10`,
                    // and it is on `0x30000f10` that `0x713` is done.
                    //
                    // **The notice goes first through the handler the game registered**, like
                    // every widget `HandleEvent` in BREW. It is what turns on the focus state of the
                    // game's widget: the home screen roller only draws the blue frame of the chosen
                    // item when its handler (`0x6089c`) receives `EVT_WDG_SETFOCUS`. Recording the
                    // focus without notifying it left the wheel with no frame at all.
                    MARK_FOCUS => {
                        self.notify_widget(this, MARK_FOCUS, id)?;
                        let parent = self.widgets.get(&this).map_or(0, |widget| widget.parent);
                        if let Some(widget) = self.widgets.get_mut(&this) {
                            widget.properties.insert(MARK_FOCUS, id);
                        }
                        if parent != 0 && parent != this {
                            if let Some(container) = self.widgets.get_mut(&parent) {
                                match id {
                                    0 => {
                                        if container.properties.get(&FOCUS_SET) == Some(&this) {
                                            container.properties.remove(&FOCUS_SET);
                                        }
                                    }
                                    _ => {
                                        container.properties.insert(FOCUS_SET, this);
                                    }
                                }
                            }
                        }
                        OK
                    }
                    // Whoever reads the focus **releases what it received** — the grid handler does
                    // the `Release` at `0x39504` —, so the answer carries a reference of ours, like
                    // every BREW interface getter. Without it, each query took away a count nobody
                    // had put there. And only a live object comes out: an address that has already
                    // gone back to the free list is worse than null.
                    FOCUS_GET => {
                        let focus = self
                            .widgets
                            .get(&this)
                            .and_then(|widget| widget.properties.get(&FOCUS_SET).copied())
                            .filter(|&focus| self.widgets.contains_key(&focus))
                            .unwrap_or(0);
                        if focus != 0 {
                            self.objects.add_ref(focus);
                        }
                        if third != 0 {
                            self.cpu.write_u32(third, focus)?;
                        }
                        OK
                    }
                    other => {
                        let who = self.cpu.read_reg(Reg::Lr);
                        let class_id = self.widgets.get(&this).map_or(0, |w| w.class_id);
                        self.missing_apis.insert(format!(
                            "IWidget::Acessador selector {other:#x} (class {class_id:#x}, from {who:#010x}, {id:#x}, {third:#x})"
                        ));
                        0
                    }
                }
            }
            _ => OK,
        };
        Ok(Some(result))
    }

    /// Serves the TrueType font. See [`Interface::Typeface`].
    ///
    /// The only method with a body is slot 4, which `0x7bfc8` calls like this:
    /// `slot4(this, a, b, c, &output)`, with the output in the **first stack argument** — the
    /// four registers are already taken. Zero is success, and what comes out is the font object,
    /// which the game hands to slot 9 of a container and then uses.
    ///
    /// We give it a widget. It is not a convenience guess: in the console's interface extension
    /// everything that goes into a screen tree is a widget, and what the game does with the object
    /// next — an `AddRef` and a slot 6 — is widget vocabulary. If it asks for something a widget
    /// does not have, the slot shows up in the report, which is how the rest of this was discovered.
    pub(super) fn typeface_call(&mut self, slot: u32) -> Result<Option<u32>, CpuError> {
        let Some(name) = Interface::Typeface.method(slot) else {
            return Ok(None);
        };
        if aee::is_placeholder(name) {
            return Ok(None);
        }
        let this = self.cpu.read_reg(Reg::R0);
        let result = match name {
            "AddRef" => self.objects.add_ref(this),
            "Release" => self.objects.release(this),
            "CriarFonte" => {
                let output = self.stack_arg(0)?;
                let font = self.new_object(Interface::Widget)?;
                if font == 0 {
                    return Ok(Some(ENOMEMORY));
                }
                self.widgets.insert(
                    font,
                    Widget {
                        visible: true,
                        ..Widget::default()
                    },
                );
                if output != 0 {
                    self.cpu.write_u32(output, font)?;
                }
                SUCCESS
            }
            _ => SUCCESS,
        };
        Ok(Some(result))
    }

    /// Where the font drawing the text came from, if there is one.
    pub fn font_source(&self) -> Option<&str> {
        self.font.as_ref().map(|f| f.source.as_str())
    }

    #[cfg(debug_assertions)]
    pub fn widgets_snapshot(&self) -> (usize, usize, usize) {
        let total = self.widgets.len();
        let largest = self
            .widgets
            .values()
            .map(|w| w.attached.len())
            .max()
            .unwrap_or(0);
        let duplicates = self
            .widgets
            .values()
            .map(|w| {
                let unique: std::collections::HashSet<_> = w.attached.iter().collect();
                w.attached.len() - unique.len()
            })
            .sum();
        (total, largest, duplicates)
    }
}

impl<C: CpuBackend> Machine<C> {
    /// `EVT_WDG_MOVEFOCUS` with one of the special values: `0` removes the focus, `1` and `2` go
    /// to the ends, `3` and `4` to the next and the previous that accepts focus. Returns whether the
    /// focus changed.
    ///
    /// Whoever leaves receives `EVT_WDG_SETFOCUS` with false, whoever enters with true — that
    /// notice is what makes the game's widget draw itself as selected. Going past the end the focus
    /// stays where it is and the answer is false, so the key goes on.
    fn move_focus(&mut self, container: u32, request: u32) -> Result<bool, CpuError> {
        const MOVE_FOCUS: u32 = 0x711;
        const SET_FOCUS: u32 = 0x700;
        let Some(widget) = self.widgets.get(&container) else {
            return Ok(false);
        };
        let children = widget.attached.clone();
        let current = widget
            .properties
            .get(&MOVE_FOCUS)
            .copied()
            .filter(|focus| children.contains(focus))
            .unwrap_or(0);
        let order: Vec<u32> = match request {
            0 => Vec::new(),
            1 => children.clone(),
            2 => children.iter().rev().copied().collect(),
            _ => {
                let position = children.iter().position(|&f| f == current);
                match (request, position) {
                    (3, Some(i)) => children[i + 1..].to_vec(),
                    (3, None) => children.clone(),
                    (_, Some(i)) => children[..i].iter().rev().copied().collect(),
                    (_, None) => children.iter().rev().copied().collect(),
                }
            }
        };
        let mut create = 0;
        for candidate in order {
            if self.accepts_focus(candidate)? {
                create = candidate;
                break;
            }
        }
        if request != 0 && create == 0 {
            return Ok(false);
        }
        if current != 0 && current != create {
            self.notify_widget(current, SET_FOCUS, 0)?;
        }
        if let Some(widget) = self.widgets.get_mut(&container) {
            match create {
                0 => widget.properties.remove(&MOVE_FOCUS),
                create => widget.properties.insert(MOVE_FOCUS, create),
            };
        }
        if create != 0 && create != current {
            self.notify_widget(create, SET_FOCUS, 1)?;
        }
        Ok(true)
    }

    /// Whether the child accepts focus, asked of its handler with `EVT_WDG_CANTAKEFOCUS`.
    ///
    /// Only a widget that draws itself and has a game handler qualifies: the game list background
    /// is a container with no drawing of its own, and our `0x702` answers "can" to everyone — making
    /// it a candidate would put the focus on it when coming back from the covers.
    fn accepts_focus(&mut self, widget: u32) -> Result<bool, CpuError> {
        const CAN_FOCUS: u32 = 0x702;
        let Some(data) = self.widgets.get(&widget) else {
            return Ok(false);
        };
        let ((callback, context), drawing) = (data.handler, data.drawing.0);
        if !data.visible || callback == 0 || drawing == 0 {
            return Ok(false);
        }
        let response = self.alloc_internal(4)?;
        if response == 0 {
            return Ok(false);
        }
        self.cpu.write_u32(response, 0)?;
        let _ = self.call_guest_nested(callback, [context, CAN_FOCUS, 0, response], QSORT_BUDGET)?;
        let focusable = self.cpu.read_u32(response)? & 0xff != 0;
        self.free(response);
        Ok(focusable)
    }

    /// Delivers an event to the handler the game registered on the widget, if there is one.
    ///
    /// A handler usually hands back to the widget what it does not handle, and that path comes back
    /// here: the per-widget lock is what keeps the same event from going round between the two.
    fn notify_widget(&mut self, widget: u32, event: u32, param: u32) -> Result<(), CpuError> {
        let Some((callback, context)) = self.widgets.get(&widget).map(|w| w.handler) else {
            return Ok(());
        };
        if callback == 0 || !self.notifying_widgets.insert(widget) {
            return Ok(());
        }
        let output = self.call_guest_nested(callback, [context, event, param, 0], QSORT_BUDGET);
        self.notifying_widgets.remove(&widget);
        output?;
        Ok(())
    }
}

impl<C: CpuBackend> Machine<C> {
    /// Releases a reference to a widget and, if it was the last one, tears it down: calls the
    /// releasers the game registered and releases, the same way, everything it held. Returns the
    /// count that is left.
    ///
    /// **Whoever stores, releases.** The children the accessor creates on its own and the ones
    /// `AdicionarFilho` hangs took a count of ours; making the widget vanish without giving it back
    /// leaks both — and the Z-Wheel in attract mode builds and tears down the intro nonstop.
    ///
    /// **Each triple's releaser is called when the widget dies**, as BREW's `HandlerDesc` says. It
    /// is what tears down what the game hung on the widget: the roller's one cancels the 40 ms
    /// animation timer (`0x2fc88`), which rearms itself. And the descent applies to the children
    /// too: releasing them only by the count left the roller out of the widget map, with no
    /// releaser called, with the timer armed over memory the Z-Wheel had already released.
    pub(super) fn release_widget(&mut self, target: u32) -> Result<u32, CpuError> {
        /// The tree comes from the game; a cycle must not trap the teardown.
        const CAP: usize = 4096;
        let remaining = self.objects.release(target);
        if remaining != 0 {
            return Ok(remaining);
        }
        let mut pending = vec![target];
        let mut visited = 0;
        while let Some(dead) = pending.pop() {
            visited += 1;
            if visited > CAP {
                break;
            }
            let Some(widget) = self.widgets.remove(&dead) else {
                continue;
            };
            self.html_pages.remove(&dead);
            self.html_scroll.remove(&dead);
            self.html_max_scroll.remove(&dead);
            for (free_fn, context) in [
                (widget.releasers.0, widget.handler.1),
                (widget.releasers.1, widget.drawing.1),
            ] {
                if free_fn != 0 {
                    let _ = self.call_guest_nested(free_fn, [context, 0, 0, 0], QSORT_BUDGET)?;
                }
            }
            for child in widget
                .children
                .into_values()
                .chain(widget.attached)
                .chain(widget.models.into_values())
            {
                if self.objects.release(child) == 0 {
                    pending.push(child);
                }
            }
        }
        Ok(0)
    }
}

/// `IContainer_GetWidget`: the neighbor of `reference` in the stack, or one end of it.
///
/// The stack goes from the first inserted (bottom) to the last (top). With no reference, `next`
/// starts from the bottom and the opposite from the top. Going past the end, `tick` wraps around;
/// without it, zero.
fn stack_neighbor(children: &[u32], reference: u32, next: bool, tick: bool) -> u32 {
    if children.is_empty() {
        return 0;
    }
    let last = children.len() - 1;
    let index = match (children.iter().position(|&w| w == reference), next) {
        (None, true) => Some(0),
        (None, false) => Some(last),
        (Some(i), true) if i < last => Some(i + 1),
        (Some(_), true) => tick.then_some(0),
        (Some(0), false) => tick.then_some(last),
        (Some(i), false) => Some(i - 1),
    };
    index.map_or(0, |i| children[i])
}

#[cfg(test)]
mod container_tests {
    use super::stack_neighbor;

    #[test]
    fn walks_the_stack_both_ways_and_stops_at_the_ends() {
        let stack = [10, 20, 30];
        assert_eq!(stack_neighbor(&stack, 0, false, false), 30, "with no reference, the top one");
        assert_eq!(stack_neighbor(&stack, 0, true, false), 10, "with no reference, the bottom one");
        assert_eq!(stack_neighbor(&stack, 30, false, false), 20);
        assert_eq!(stack_neighbor(&stack, 10, false, false), 0, "the end finishes the loop");
        assert_eq!(stack_neighbor(&stack, 10, false, true), 30, "with wrap, it wraps around");
        assert_eq!(stack_neighbor(&stack, 20, true, false), 30);
        assert_eq!(stack_neighbor(&[], 0, true, true), 0);
    }
}

/// Distance between the edge of the HTML widget and the text, in pixels.
const HTML_MARGIN: i32 = 8;

/// Where the placeholder HTML of the help screen lives: on the emulated device, next to the other
/// Z-Wheel files.
pub fn html_path() -> std::path::PathBuf {
    crate::loader::archive::device_dir()
        .join("z-wheel")
        .join("ajuda.html")
}

/// The initial content of the placeholder, written the first time the help screen appears.
const DEFAULT_HTML: &str = "<html>
<body>
<h1>Help</h1>
<p>This page is a Zeebx placeholder.</p>
<p>The console showed the Z-Wheel help pages here. Until the HTML viewer exists in the
emulator, the text comes from this file, which can be edited freely.</p>
</body>
</html>
";

/// The placeholder text, creating the file with the default content when it does not exist.
fn read_placeholder_html() -> String {
    let path = html_path();
    match std::fs::read_to_string(&path) {
        Ok(text) => text,
        Err(_) => {
            if let Some(dir) = path.parent() {
                let _ = std::fs::create_dir_all(dir);
            }
            let _ = std::fs::write(&path, DEFAULT_HTML);
            DEFAULT_HTML.to_string()
        }
    }
}

/// The paragraphs of a simple HTML, as running text.
///
/// It is not a parser: blocks (`p`, `br`, `div`, `li`, headings) become paragraph breaks, the rest
/// of the tags vanish, whitespace is collapsed and the most common entities are translated. It is
/// enough for a hand-written help text.
pub fn html_text(html: &str) -> Vec<String> {
    const BLOCKS: [&str; 14] = [
        "p", "/p", "br", "br/", "div", "/div", "li", "h1", "/h1", "h2", "/h2", "/li", "center",
        "/center",
    ];
    let mut paragraphs = Vec::new();
    let mut current = String::new();
    let mut rest = html;
    let close = |current: &mut String, paragraphs: &mut Vec<String>| {
        let clean = current.split_whitespace().collect::<Vec<_>>().join(" ");
        if !clean.is_empty() {
            paragraphs.push(clean);
        }
        current.clear();
    };
    while let Some(open) = rest.find('<') {
        current.push_str(&rest[..open]);
        let Some(end) = rest[open..].find('>') else {
            rest = "";
            break;
        };
        let tag = rest[open + 1..open + end].trim().to_ascii_lowercase();
        let name = tag.split_whitespace().next().unwrap_or("").trim_end_matches('/');
        if BLOCKS.contains(&name) || BLOCKS.contains(&tag.as_str()) {
            close(&mut current, &mut paragraphs);
        }
        rest = &rest[open + end + 1..];
    }
    current.push_str(rest);
    close(&mut current, &mut paragraphs);
    paragraphs
        .into_iter()
        .map(|p| decode_entities(&p))
        .collect()
}

/// The text of a page as it came from the file: UTF-8 when it is, and Latin-1 when it is not,
/// which is how the console's old pages were written.
fn decode_page(bytes: &[u8]) -> String {
    match std::str::from_utf8(bytes) {
        Ok(text) => text.to_string(),
        Err(_) => bytes.iter().map(|&b| char::from(b)).collect(),
    }
}

/// Replaces HTML entities with the character. The Z-Wheel pages write accents this way
/// (`n&atilde;o`, `est&aacute;`); the named ones missing here stay as they came.
fn decode_entities(text: &str) -> String {
    const NAMED_ENTITIES: &[(&str, char)] = &[
        ("nbsp", ' '), ("lt", '<'), ("gt", '>'), ("quot", '"'), ("amp", '&'), ("apos", '\''),
        ("aacute", 'á'), ("agrave", 'à'), ("acirc", 'â'), ("atilde", 'ã'), ("auml", 'ä'),
        ("eacute", 'é'), ("egrave", 'è'), ("ecirc", 'ê'), ("iacute", 'í'), ("icirc", 'î'),
        ("oacute", 'ó'), ("ocirc", 'ô'), ("otilde", 'õ'), ("ouml", 'ö'), ("uacute", 'ú'),
        ("uuml", 'ü'), ("ccedil", 'ç'), ("ntilde", 'ñ'), ("Aacute", 'Á'), ("Agrave", 'À'),
        ("Acirc", 'Â'), ("Atilde", 'Ã'), ("Eacute", 'É'), ("Ecirc", 'Ê'), ("Iacute", 'Í'),
        ("Oacute", 'Ó'), ("Ocirc", 'Ô'), ("Otilde", 'Õ'), ("Uacute", 'Ú'), ("Ccedil", 'Ç'),
        ("Ntilde", 'Ñ'), ("ordm", 'º'), ("ordf", 'ª'), ("copy", '©'), ("reg", '®'),
        ("deg", '°'), ("iexcl", '¡'), ("iquest", '¿'), ("trade", '™'), ("hellip", '…'),
        ("ndash", '–'), ("mdash", '—'), ("laquo", '«'), ("raquo", '»'), ("bull", '•'),
    ];
    let mut output = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(start) = rest.find('&') {
        output.push_str(&rest[..start]);
        let after = &rest[start + 1..];
        let end = after.find(';').filter(|&n| n <= 8);
        let character = end.and_then(|n| {
            let name = &after[..n];
            match name.strip_prefix('#') {
                Some(number) => match number.strip_prefix(['x', 'X']) {
                    Some(hex) => u32::from_str_radix(hex, 16).ok(),
                    None => number.parse().ok(),
                }
                .and_then(char::from_u32),
                None => NAMED_ENTITIES.iter().find(|(n, _)| *n == name).map(|(_, c)| *c),
            }
        });
        match (end, character) {
            (Some(n), Some(c)) => {
                output.push(c);
                rest = &after[n + 1..];
            }
            _ => {
                output.push('&');
                rest = after;
            }
        }
    }
    output.push_str(rest);
    output
}

/// Breaks a paragraph into lines that fit in `width`, by the font's measure.
fn wrap_lines(paragraph: &str, width: u32, measure: impl Fn(&str) -> u32) -> Vec<String> {
    let mut lines = Vec::new();
    let mut line = String::new();
    for word in paragraph.split_whitespace() {
        let candidate = match line.is_empty() {
            true => word.to_string(),
            false => format!("{line} {word}"),
        };
        if !line.is_empty() && measure(&candidate) > width {
            lines.push(std::mem::take(&mut line));
            line = word.to_string();
        } else {
            line = candidate;
        }
    }
    if !line.is_empty() {
        lines.push(line);
    }
    lines
}

#[cfg(test)]
mod html_tests {
    use super::{wrap_lines, html_text};

    #[test]
    fn blocks_become_paragraphs_and_tags_disappear() {
        let html = "<h1>Ajuda</h1><p>Um <b>texto</b>\n  com   espaços &amp; tags.</p>linha<br>outra";
        assert_eq!(
            html_text(html),
            ["Ajuda", "Um texto com espaços & tags.", "linha", "outra"]
        );
    }

    #[test]
    fn accented_entities_become_letters() {
        let html = "<center>Iniciando</center>\n<p>n&atilde;o est&aacute; &#233; &amp;c &foo; a&b</p>";
        assert_eq!(html_text(html), ["Iniciando", "não está é &c &foo; a&b"]);
    }

    #[test]
    fn lines_wrap_at_the_width() {
        let lines = wrap_lines("aa bb cc", 5, |t| t.len() as u32);
        assert_eq!(lines, ["aa bb", "cc"]);
    }
}
