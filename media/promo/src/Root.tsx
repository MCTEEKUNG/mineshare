import {Composition} from 'remotion';
import {DeskStory} from './Composition';

export const RemotionRoot: React.FC = () => {
  return (
    <>
      <Composition id="DeskStory" component={DeskStory} durationInFrames={540} fps={30} width={1920} height={1080} />
    </>
  );
};
