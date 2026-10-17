import React from "react";
import { Composition } from "remotion";
import "./fonts";
import { ShodhFilm } from "./film";
import { FPS } from "./theme";

/** The film runs the whole edited track. */
const FILM_FRAMES = Math.round(121.84 * FPS);

export const Root: React.FC = () => (
  <Composition id="ShodhFilm" component={ShodhFilm} durationInFrames={FILM_FRAMES} fps={FPS} width={1920} height={1080} />
);
